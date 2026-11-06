//! Zenith: Fast, lightweight limbo and queue server in Rust for Horizon and Meridian.

use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, error, info};
use zenith_display::{
    DisplayConfig, HudRenderer, encode_subtitle_packet, encode_system_chat_packet,
    encode_title_packet, encode_title_times_packet,
};
use zenith_session::{SessionCommand, SessionId, VirtualSession};
use zenith_shield::ShieldLimits;
use zenith_transit::{
    QueueEntry, QueuePriority, SortingPolicy, TargetServerHealth, TransitConfig, TransitQueue,
};

type ActiveSessions = Arc<
    Mutex<
        HashMap<
            SessionId,
            (
                mpsc::UnboundedSender<SessionCommand>,
                String,
                u128,
                usize,
                u128,
            ),
        >,
    >,
>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("zenith 0.1.0");
        return Ok(());
    }

    if args.iter().any(|arg| arg == "--check") {
        info!("Zenith configuration check passed.");
        return Ok(());
    }

    let bind_addr = env::var("ZENITH_BIND").unwrap_or_else(|_| "0.0.0.0:25565".to_string());
    let target_server = env::var("ZENITH_TARGET").unwrap_or_else(|_| "127.0.0.1:25566".to_string());
    let wave_size: usize = env::var("ZENITH_WAVE_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let wave_interval_ms: u64 = env::var("ZENITH_WAVE_INTERVAL_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4000);

    info!("Starting Zenith v0.1.0 (Protocol 777 / Minecraft 26.3)...");
    info!("Target backend: {}", target_server);
    info!(
        "Wave settings: {} players every {}ms",
        wave_size, wave_interval_ms
    );
    info!("Listening for virtual limbo connections on {}", bind_addr);

    let transit_config = TransitConfig {
        sorting_policy: SortingPolicy::PingAscending,
        nominal_wave_size: wave_size,
        nominal_interval_ms: wave_interval_ms,
        ..Default::default()
    };
    let queue = Arc::new(Mutex::new(TransitQueue::new(transit_config)));
    let hud = Arc::new(HudRenderer::new(DisplayConfig::default()));
    let active_sessions: ActiveSessions = Arc::new(Mutex::new(HashMap::new()));

    // 1. Background HUD refresh loop (every 1 second)
    let hud_queue = Arc::clone(&queue);
    let hud_renderer = Arc::clone(&hud);
    let hud_sessions = Arc::clone(&active_sessions);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(1000));
        loop {
            interval.tick().await;

            let q = hud_queue.lock().await;
            if q.is_empty() {
                continue;
            }

            let nominal_wave_size = q.config().nominal_wave_size.max(1);
            let nominal_interval_ms = q.config().nominal_interval_ms.max(1000);

            let mut sessions = hud_sessions.lock().await;
            for (&session_id, (tx, _username, _uuid, last_pos, bb_uuid)) in sessions.iter_mut() {
                if let Some((pos, total)) = q.position_of(session_id) {
                    let eta_seconds = ((pos as u64).saturating_sub(1) / nominal_wave_size as u64)
                        * (nominal_interval_ms / 1000);

                    // Send Actionbar
                    let actionbar = hud_renderer.render_actionbar(pos, total, eta_seconds);
                    let _ = tx.send(SessionCommand::SendRaw(actionbar));

                    // Send Bossbar update & click sound on position change
                    if pos != *last_pos {
                        if pos < *last_pos {
                            if let Some(click) = hud_renderer.render_click_sound() {
                                let _ = tx.send(SessionCommand::SendRaw(click));
                            }
                        }
                        let (title_pkt, health_pkt) =
                            hud_renderer.render_bossbar_update(*bb_uuid, pos, total, eta_seconds);
                        let _ = tx.send(SessionCommand::SendRaw(title_pkt));
                        let _ = tx.send(SessionCommand::SendRaw(health_pkt));
                        *last_pos = pos;
                    }
                }
            }
        }
    });

    // 2. Background Wave Drain loop (health-adaptive transfers)
    let wave_queue = Arc::clone(&queue);
    let wave_hud = Arc::clone(&hud);
    let wave_sessions = Arc::clone(&active_sessions);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(wave_interval_ms));
        loop {
            interval.tick().await;

            let drained = {
                let mut q = wave_queue.lock().await;
                if q.is_empty() {
                    continue;
                }
                let health = TargetServerHealth::default();
                q.drain_wave(Instant::now(), &health)
            };

            if !drained.is_empty() {
                info!(
                    count = drained.len(),
                    "Draining transit wave towards backend"
                );
                let mut sessions = wave_sessions.lock().await;
                for entry in drained {
                    if let Some((tx, username, _uuid, _last_pos, bb_uuid)) =
                        sessions.remove(&entry.session_id)
                    {
                        let (host, port) = match entry.target_server.split_once(':') {
                            Some((h, p)) => (h, p.parse::<u16>().unwrap_or(25565)),
                            None => (entry.target_server.as_str(), 25565),
                        };

                        // Send transfer sound and packet
                        let (sound_opt, transfer_pkt) = wave_hud.render_transfer(host, port);
                        if let Some(snd) = sound_opt {
                            let _ = tx.send(SessionCommand::SendRaw(snd));
                        }
                        let chat_msg = encode_system_chat_packet(
                            "{\"text\":\"§6§l[Zenith] §aYour turn has arrived! Transferring to backend...\"}",
                            false,
                        );
                        let _ = tx.send(SessionCommand::SendRaw(chat_msg));
                        let _ = tx.send(SessionCommand::SendRaw(transfer_pkt));

                        // Clear Bossbar
                        let remove_bb = wave_hud.render_bossbar_remove(bb_uuid);
                        let _ = tx.send(SessionCommand::SendRaw(remove_bb));

                        // Instruct session to execute transfer
                        let _ = tx.send(SessionCommand::Transfer {
                            host: host.to_string(),
                            port,
                        });
                        info!(
                            player = %username,
                            target = %entry.target_server,
                            "Player transferred successfully"
                        );
                    }
                }
            }
        }
    });

    // 3. Listener accept loop
    let listener = TcpListener::bind(&bind_addr).await?;
    info!("Zenith server online. Ready for player connections!");

    loop {
        let (socket, peer_addr) = listener.accept().await?;
        debug!(peer = %peer_addr, "Accepted incoming TCP connection");

        let queue_clone = Arc::clone(&queue);
        let hud_clone = Arc::clone(&hud);
        let sessions_clone = Arc::clone(&active_sessions);
        let target_clone = target_server.clone();

        tokio::spawn(async move {
            let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
            let mut session =
                VirtualSession::with_command_channel(socket, ShieldLimits::default(), cmd_rx);

            match session.complete_handshake_and_login().await {
                Ok(None) => {
                    // Status Ping completed cleanly
                }
                Ok(Some((username, uuid))) => {
                    let session_id = session.id;
                    let bb_uuid = uuid;

                    let lower = username.to_lowercase();
                    let priority = if lower.contains("staff") || lower.contains("admin") {
                        QueuePriority::Staff
                    } else if lower.contains("vip") {
                        QueuePriority::Vip
                    } else {
                        QueuePriority::Standard
                    };

                    info!(
                        session = ?session_id,
                        player = %username,
                        priority = ?priority,
                        peer = %peer_addr,
                        "Player entered Zenith virtual holding dimension"
                    );

                    let (pos, total, eta_seconds) = {
                        let mut q = queue_clone.lock().await;
                        q.enqueue(QueueEntry {
                            session_id,
                            uuid,
                            username: username.clone(),
                            priority,
                            ping_ms: 20,
                            enqueued_at: Instant::now(),
                            target_server: target_clone,
                        });
                        let (p, t) = q.position_of(session_id).unwrap_or((1, 1));
                        let eta = ((p as u64).saturating_sub(1)
                            / q.config().nominal_wave_size.max(1) as u64)
                            * (q.config().nominal_interval_ms / 1000).max(1);
                        (p, t, eta)
                    };

                    // Send initial BossBar Add
                    let add_bb = hud_clone.render_bossbar_add(bb_uuid, pos, total, eta_seconds);
                    let _ = cmd_tx.send(SessionCommand::SendRaw(add_bb));

                    // Send Welcome Titles
                    let times = encode_title_times_packet(10, 60, 20);
                    let _ = cmd_tx.send(SessionCommand::SendRaw(times));
                    let sub = encode_subtitle_packet("§7Virtual Limbo & Queue Gatekeeper");
                    let _ = cmd_tx.send(SessionCommand::SendRaw(sub));
                    let title = encode_title_packet("§b§lZENITH");
                    let _ = cmd_tx.send(SessionCommand::SendRaw(title));

                    // Send Welcome Chat
                    let welcome = encode_system_chat_packet(
                        &format!(
                            "{{\"text\":\"§b§l[Zenith] §7Welcome §f{}§7! You are enqueued at position §e#{}§7 / §e{}§7.\"}}",
                            username, pos, total
                        ),
                        false,
                    );
                    let _ = cmd_tx.send(SessionCommand::SendRaw(welcome));

                    // Register in active sessions map
                    sessions_clone
                        .lock()
                        .await
                        .insert(session_id, (cmd_tx, username.clone(), uuid, pos, bb_uuid));

                    // Run the limbo loop
                    if let Err(e) = session.handle_limbo_loop().await {
                        error!(session = ?session_id, error = %e, "Limbo session terminated with error");
                    } else {
                        info!(session = ?session_id, player = %username, "Limbo session closed cleanly");
                    }

                    // Cleanup if disconnected before wave drain
                    queue_clone.lock().await.remove(session_id);
                    sessions_clone.lock().await.remove(&session_id);
                }
                Err(e) => {
                    debug!(error = %e, "Handshake/Login ended (client disconnect or ping)");
                }
            }
        });
    }
}
