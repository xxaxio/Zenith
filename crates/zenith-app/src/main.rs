//! Zenith: Fast, lightweight limbo and queue server in Rust for Horizon and Meridian.

use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, error, info};
use zenith_display::{
    DisplayConfig, HudRenderer, encode_subtitle_packet, encode_system_chat_packet,
    encode_title_packet, encode_title_times_packet,
};
use zenith_session::{SessionCommand, SessionEvent, SessionId, VirtualSession};
use zenith_shield::ShieldLimits;
use zenith_transit::{
    GroupManager, QueueEntry, SortingPolicy, TargetServerHealth, TransitConfig, TransitQueue,
};

/// Zenith operational modes for testing and production deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZenithMode {
    /// Infinite virtual holding: players remain in Limbo, explore, chat, and test commands (/transfer, /ping, etc.).
    Limbo,
    /// Transit queue: players are held and drained in waves to the target backend server.
    Queue,
    /// Stress harness: high-throughput bot load testing.
    Stress,
}

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

struct CommandContext {
    queue: Arc<Mutex<TransitQueue>>,
    group_mgr: Arc<Mutex<GroupManager>>,
    sessions: ActiveSessions,
    is_queue_active: Arc<AtomicBool>,
    target_server: String,
    hud: Arc<HudRenderer>,
}

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
    let mode_str = env::var("ZENITH_MODE").unwrap_or_else(|_| "limbo".to_string());
    let mode = match mode_str.to_lowercase().as_str() {
        "queue" | "transit" => ZenithMode::Queue,
        "stress" | "benchmark" => ZenithMode::Stress,
        _ => ZenithMode::Limbo,
    };

    info!("Starting Zenith v0.1.0 (Protocol 777 / Minecraft 26.3)...");
    info!("Operating Mode: {:?}", mode);
    info!("Target backend: {}", target_server);
    if mode == ZenithMode::Queue {
        info!(
            "Wave settings: {} players every {}ms",
            wave_size, wave_interval_ms
        );
    } else {
        info!(
            "Limbo showcase active: auto-transfer disabled. Players stay in limbo; type /transfer to transfer."
        );
    }
    info!("Listening for virtual limbo connections on {}", bind_addr);

    let transit_config = TransitConfig {
        sorting_policy: SortingPolicy::PingAscending,
        nominal_wave_size: wave_size,
        nominal_interval_ms: wave_interval_ms,
        ..Default::default()
    };
    let queue = Arc::new(Mutex::new(TransitQueue::new(transit_config)));
    let group_manager = Arc::new(Mutex::new(GroupManager::default()));
    let hud = Arc::new(HudRenderer::new(DisplayConfig::default()));
    let active_sessions: ActiveSessions = Arc::new(Mutex::new(HashMap::new()));
    let is_queue_active = Arc::new(AtomicBool::new(mode == ZenithMode::Queue));

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<SessionEvent>();

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

            let total = q.len();
            let nominal_wave_size = q.config().nominal_wave_size.max(1);
            let nominal_interval_ms = q.config().nominal_interval_ms.max(1000);

            let mut sessions = hud_sessions.lock().await;
            for (&session_id, (tx, _username, _uuid, last_pos, bb_uuid)) in sessions.iter_mut() {
                if mode == ZenithMode::Limbo {
                    let actionbar = hud_renderer.render_limbo_actionbar();
                    let _ = tx.send(SessionCommand::SendRaw(actionbar));

                    if *last_pos != total {
                        let (title_pkt, health_pkt) =
                            hud_renderer.render_limbo_bossbar_update(*bb_uuid, total);
                        let _ = tx.send(SessionCommand::SendRaw(title_pkt));
                        let _ = tx.send(SessionCommand::SendRaw(health_pkt));
                        *last_pos = total;
                    }
                } else if let Some((pos, total)) = q.position_of(session_id) {
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

    // 2. Background Wave Drain loop (health-adaptive transfers - only active in Queue mode)
    if mode == ZenithMode::Queue {
        let wave_queue = Arc::clone(&queue);
        let wave_hud = Arc::clone(&hud);
        let wave_sessions = Arc::clone(&active_sessions);
        let wave_active_flag = Arc::clone(&is_queue_active);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(wave_interval_ms));
            loop {
                interval.tick().await;

                if !wave_active_flag.load(Ordering::SeqCst) {
                    continue;
                }

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
                                "§b§lᴢᴇɴɪᴛʜ §8» §aVotre tour est arrivé ! Transfert en cours...",
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
    }

    // 3. Central Event & Command Coordinator Loop
    let cmd_context = Arc::new(CommandContext {
        queue: Arc::clone(&queue),
        group_mgr: Arc::clone(&group_manager),
        sessions: Arc::clone(&active_sessions),
        is_queue_active: Arc::clone(&is_queue_active),
        target_server: target_server.clone(),
        hud: Arc::clone(&hud),
    });

    let loop_ctx = Arc::clone(&cmd_context);
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                SessionEvent::Command {
                    session_id,
                    username,
                    command,
                    args,
                } => {
                    handle_command(session_id, &username, &command, &args, &loop_ctx).await;
                }
                SessionEvent::Disconnected { session_id } => {
                    loop_ctx.queue.lock().await.remove(session_id);
                    loop_ctx.sessions.lock().await.remove(&session_id);
                }
            }
        }
    });

    // 4. Listener accept loop
    let listener = TcpListener::bind(&bind_addr).await?;
    info!("Zenith server online. Ready for player connections!");

    loop {
        let (socket, peer_addr) = listener.accept().await?;
        debug!(peer = %peer_addr, "Accepted incoming TCP connection");

        let queue_clone = Arc::clone(&queue);
        let group_clone = Arc::clone(&group_manager);
        let hud_clone = Arc::clone(&hud);
        let sessions_clone = Arc::clone(&active_sessions);
        let target_clone = target_server.clone();
        let event_tx_clone = event_tx.clone();

        tokio::spawn(async move {
            let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
            let mut session =
                VirtualSession::with_command_channel(socket, ShieldLimits::default(), cmd_rx);
            session.set_target_server(target_clone.clone());
            session.set_event_sender(event_tx_clone);

            match session.complete_handshake_and_login().await {
                Ok(None) => {
                    // Status Ping completed cleanly
                }
                Ok(Some((username, uuid))) => {
                    let session_id = session.id;
                    let bb_uuid = uuid;

                    // Resolve priority from GroupManager (staff, vip, owner)
                    let priority = group_clone.lock().await.priority_of(&username);

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
                    if mode == ZenithMode::Limbo {
                        let add_bb = hud_clone.render_limbo_bossbar_add(bb_uuid, total);
                        let _ = cmd_tx.send(SessionCommand::SendRaw(add_bb));
                    } else {
                        let add_bb = hud_clone.render_bossbar_add(bb_uuid, pos, total, eta_seconds);
                        let _ = cmd_tx.send(SessionCommand::SendRaw(add_bb));
                    }

                    // Send sleek, minimal welcome title (fade in, short display)
                    let times = encode_title_times_packet(10, 40, 10);
                    let _ = cmd_tx.send(SessionCommand::SendRaw(times));
                    let sub = encode_subtitle_packet(if mode == ZenithMode::Limbo {
                        "§7Sas de rétention"
                    } else {
                        "§7File d'attente"
                    });
                    let _ = cmd_tx.send(SessionCommand::SendRaw(sub));
                    let title = encode_title_packet("§b§lᴢᴇɴɪᴛʜ");
                    let _ = cmd_tx.send(SessionCommand::SendRaw(title));

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

async fn handle_command(
    session_id: SessionId,
    username: &str,
    cmd: &str,
    args: &[String],
    ctx: &CommandContext,
) {
    let tx = {
        let s = ctx.sessions.lock().await;
        s.get(&session_id).map(|(tx, _, _, _, _)| tx.clone())
    };
    let Some(tx) = tx else { return };

    let is_staff = ctx.group_mgr.lock().await.is_staff(username);

    match cmd {
        "transfer" | "join" | "play" => {
            let (host, port) = match ctx.target_server.split_once(':') {
                Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(25565)),
                None => (ctx.target_server.clone(), 25565),
            };

            let (sound_opt, transfer_pkt) = ctx.hud.render_transfer(&host, port);
            if let Some(snd) = sound_opt {
                let _ = tx.send(SessionCommand::SendRaw(snd));
            }
            let msg = encode_system_chat_packet(
                &format!("§b§lᴢᴇɴɪᴛʜ §8» §aTransfert vers §e{}:{}§a...", host, port),
                false,
            );
            let _ = tx.send(SessionCommand::SendRaw(msg));
            let _ = tx.send(SessionCommand::SendRaw(transfer_pkt));
            let _ = tx.send(SessionCommand::Transfer { host, port });
        }
        "queue" => {
            if !is_staff {
                let msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cVous n'avez pas la permission d'utiliser cette commande.",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
                return;
            }

            let was_active = ctx.is_queue_active.load(Ordering::SeqCst);
            let new_active = !was_active;
            ctx.is_queue_active.store(new_active, Ordering::SeqCst);
            ctx.queue.lock().await.toggle_pause();

            let sess = ctx.sessions.lock().await;
            if new_active {
                info!(by = %username, "Queue OPENED by staff member");
                let times = encode_title_times_packet(5, 50, 15);
                let sub = encode_subtitle_packet("§b§lC'EST PARTI ! §7La file est ouverte");
                let title = encode_title_packet("§a▶");
                let chat = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §aLa file d'attente est maintenant §2§lOUVERTE §a!",
                    false,
                );

                for (s_tx, _, _, _, _) in sess.values() {
                    let _ = s_tx.send(SessionCommand::SendRaw(times.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(sub.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(title.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(chat.clone()));
                }
            } else {
                info!(by = %username, "Queue PAUSED by staff member");
                let times = encode_title_times_packet(5, 50, 15);
                let sub = encode_subtitle_packet("§c§lFILE EN PAUSE §7Ouverture suspendue");
                let title = encode_title_packet("§c⏸");
                let chat = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cLa file d'attente est maintenant §4§lEN PAUSE §c!",
                    false,
                );

                for (s_tx, _, _, _, _) in sess.values() {
                    let _ = s_tx.send(SessionCommand::SendRaw(times.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(sub.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(title.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(chat.clone()));
                }
            }
        }
        "queueskip" | "skip" => {
            if !is_staff {
                let msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cVous n'avez pas la permission d'utiliser cette commande.",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
                return;
            }

            if args.is_empty() {
                let msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cUtilisation: /queueskip <joueur>",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
                return;
            }

            let target_name = &args[0];
            let mut target_found = None;

            {
                let s = ctx.sessions.lock().await;
                for (&s_id, (s_tx, u_name, _, _, bb_uuid)) in s.iter() {
                    if u_name.eq_ignore_ascii_case(target_name) {
                        target_found = Some((s_id, s_tx.clone(), u_name.clone(), *bb_uuid));
                        break;
                    }
                }
            }

            if let Some((target_id, target_tx, target_actual_name, target_bb_uuid)) = target_found {
                info!(by = %username, target = %target_actual_name, "Admin executed queueskip");

                let (host, port) = match ctx.target_server.split_once(':') {
                    Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(25565)),
                    None => (ctx.target_server.clone(), 25565),
                };

                let (sound_opt, transfer_pkt) = ctx.hud.render_transfer(&host, port);
                if let Some(snd) = sound_opt {
                    let _ = target_tx.send(SessionCommand::SendRaw(snd));
                }
                let target_msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §aTransfert forcé vers le serveur de jeu !",
                    false,
                );
                let _ = target_tx.send(SessionCommand::SendRaw(target_msg));
                let _ = target_tx.send(SessionCommand::SendRaw(transfer_pkt));
                let remove_bb = ctx.hud.render_bossbar_remove(target_bb_uuid);
                let _ = target_tx.send(SessionCommand::SendRaw(remove_bb));
                let _ = target_tx.send(SessionCommand::Transfer {
                    host: host.clone(),
                    port,
                });

                ctx.queue.lock().await.remove(target_id);
                ctx.sessions.lock().await.remove(&target_id);

                let reply = encode_system_chat_packet(
                    &format!(
                        "§b§lᴢᴇɴɪᴛʜ §8» §aTransfert forcé de §e{} §aeffectué avec succès.",
                        target_actual_name
                    ),
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(reply));
            } else {
                let msg = encode_system_chat_packet(
                    &format!(
                        "§b§lᴢᴇɴɪᴛʜ §8» §cLe joueur '{}' n'est pas dans la file d'attente.",
                        target_name
                    ),
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
            }
        }
        "group" => {
            if !is_staff {
                let msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cVous n'avez pas la permission d'utiliser cette commande.",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
                return;
            }

            let subcmd = args.first().map(|s| s.as_str()).unwrap_or("");
            match subcmd {
                "add" => {
                    if args.len() < 3 {
                        let msg = encode_system_chat_packet(
                            "§b§lᴢᴇɴɪᴛʜ §8» §cUtilisation: /group add <owner|staff|vip> <joueur>",
                            false,
                        );
                        let _ = tx.send(SessionCommand::SendRaw(msg));
                        return;
                    }
                    let group = &args[1];
                    let player = &args[2];

                    let mut gm = ctx.group_mgr.lock().await;
                    match gm.add(group, player) {
                        Ok(()) => {
                            let new_prio = gm.priority_of(player);
                            let mut q = ctx.queue.lock().await;
                            if let Some(target_sid) = q.session_id_of(player) {
                                q.update_priority(target_sid, new_prio);
                            }
                            info!(by = %username, target = %player, group = %group, "Added player to group");
                            let msg = encode_system_chat_packet(
                                &format!(
                                    "§b§lᴢᴇɴɪᴛʜ §8» §aJoueur §e{} §aajouté au groupe §b{}§a.",
                                    player, group
                                ),
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                        }
                        Err(e) => {
                            let msg = encode_system_chat_packet(
                                &format!("§b§lᴢᴇɴɪᴛʜ §8» §c{}", e),
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                        }
                    }
                }
                "remove" => {
                    if args.len() < 3 {
                        let msg = encode_system_chat_packet(
                            "§b§lᴢᴇɴɪᴛʜ §8» §cUtilisation: /group remove <owner|staff|vip> <joueur>",
                            false,
                        );
                        let _ = tx.send(SessionCommand::SendRaw(msg));
                        return;
                    }
                    let group = &args[1];
                    let player = &args[2];

                    let mut gm = ctx.group_mgr.lock().await;
                    match gm.remove(group, player) {
                        Ok(()) => {
                            let new_prio = gm.priority_of(player);
                            let mut q = ctx.queue.lock().await;
                            if let Some(target_sid) = q.session_id_of(player) {
                                q.update_priority(target_sid, new_prio);
                            }
                            info!(by = %username, target = %player, group = %group, "Removed player from group");
                            let msg = encode_system_chat_packet(
                                &format!(
                                    "§b§lᴢᴇɴɪᴛʜ §8» §cJoueur §e{} §cretiré du groupe §b{}§c.",
                                    player, group
                                ),
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                        }
                        Err(e) => {
                            let msg = encode_system_chat_packet(
                                &format!("§b§lᴢᴇɴɪᴛʜ §8» §c{}", e),
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                        }
                    }
                }
                "list" => {
                    let gm = ctx.group_mgr.lock().await;
                    let msg1 = encode_system_chat_packet(
                        "§b§l=== Groupes & Permissions Zenith ===",
                        false,
                    );
                    let msg2 = encode_system_chat_packet(
                        &format!("§eOwners: §f{}", gm.owners().join(", ")),
                        false,
                    );
                    let msg3 = encode_system_chat_packet(
                        &format!("§bStaff:  §f{}", gm.staff_members().join(", ")),
                        false,
                    );
                    let vips = gm.vip_members();
                    let vips_str = if vips.is_empty() {
                        "aucun".to_string()
                    } else {
                        vips.join(", ")
                    };
                    let msg4 =
                        encode_system_chat_packet(&format!("§aVIPs:   §f{}", vips_str), false);
                    let _ = tx.send(SessionCommand::SendRaw(msg1));
                    let _ = tx.send(SessionCommand::SendRaw(msg2));
                    let _ = tx.send(SessionCommand::SendRaw(msg3));
                    let _ = tx.send(SessionCommand::SendRaw(msg4));
                }
                _ => {
                    let msg = encode_system_chat_packet(
                        "§b§lᴢᴇɴɪᴛʜ §8» §cUsage: /group <add|remove|list> [owner|staff|vip] [joueur]",
                        false,
                    );
                    let _ = tx.send(SessionCommand::SendRaw(msg));
                }
            }
        }
        "queueinfo" | "info" => {
            let q = ctx.queue.lock().await;
            let (pos, total) = q.position_of(session_id).unwrap_or((1, q.len().max(1)));
            let nominal_wave_size = q.config().nominal_wave_size.max(1);
            let nominal_interval_ms = q.config().nominal_interval_ms.max(1000);
            let eta_seconds = ((pos as u64).saturating_sub(1) / nominal_wave_size as u64)
                * (nominal_interval_ms / 1000);
            let eta_str = zenith_display::format_eta(eta_seconds);

            let status_str = if ctx.is_queue_active.load(Ordering::SeqCst) {
                "§aOuverte"
            } else {
                "§cEn pause"
            };

            let m1 = encode_system_chat_packet(
                "§8§m                                                  §r",
                false,
            );
            let m2 = encode_system_chat_packet("§b§l  ᴢᴇɴɪᴛʜ §8» §fFile d'Attente", false);
            let m3 = encode_system_chat_packet(
                &format!("§7  Statut: {} §7| Joueurs: §e{}", status_str, total),
                false,
            );
            let m4 = encode_system_chat_packet(
                &format!(
                    "§7  Votre position: §e#{} §8/ §e{} §7(attente: §a~{}§7)",
                    pos, total, eta_str
                ),
                false,
            );
            let m5 = encode_system_chat_packet(
                "§8§m                                                  §r",
                false,
            );

            let _ = tx.send(SessionCommand::SendRaw(m1));
            let _ = tx.send(SessionCommand::SendRaw(m2));
            let _ = tx.send(SessionCommand::SendRaw(m3));
            let _ = tx.send(SessionCommand::SendRaw(m4));
            let _ = tx.send(SessionCommand::SendRaw(m5));
        }
        "ping" => {
            let msg = encode_system_chat_packet("§b§lᴢᴇɴɪᴛʜ §8» §7Latence réseau: §a~15ms", false);
            let _ = tx.send(SessionCommand::SendRaw(msg));
        }
        "help" => {
            let m1 = encode_system_chat_packet(
                "§8§m                                                  §r",
                false,
            );
            let m2 = encode_system_chat_packet("§b§l  ᴢᴇɴɪᴛʜ §8» §fCommandes Disponibles", false);
            let m3 = encode_system_chat_packet(
                "  §b/transfer §8- §7Rejoindre immédiatement le serveur",
                false,
            );
            let m4 = encode_system_chat_packet(
                "  §e/info     §8- §7Consulter votre position et temps estimé",
                false,
            );
            let m5 = encode_system_chat_packet(
                "  §e/ping     §8- §7Afficher votre latence réseau",
                false,
            );
            let _ = tx.send(SessionCommand::SendRaw(m1));
            let _ = tx.send(SessionCommand::SendRaw(m2));
            let _ = tx.send(SessionCommand::SendRaw(m3));
            let _ = tx.send(SessionCommand::SendRaw(m4));
            let _ = tx.send(SessionCommand::SendRaw(m5));

            if is_staff {
                let m_staff = encode_system_chat_packet(
                    "  §c/queue         §8- §7Ouvrir ou mettre en pause la file",
                    false,
                );
                let m_skip = encode_system_chat_packet(
                    "  §c/queueskip <j> §8- §7Transférer de force un joueur",
                    false,
                );
                let m_grp = encode_system_chat_packet(
                    "  §c/group <a|r|l> §8- §7Gérer les permissions (owner/staff/vip)",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(m_staff));
                let _ = tx.send(SessionCommand::SendRaw(m_skip));
                let _ = tx.send(SessionCommand::SendRaw(m_grp));
            }

            let m_end = encode_system_chat_packet(
                "§8§m                                                  §r",
                false,
            );
            let _ = tx.send(SessionCommand::SendRaw(m_end));
        }
        _ => {}
    }
}
