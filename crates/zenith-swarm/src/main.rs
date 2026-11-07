//! Zenith Swarm: Real TCP Minecraft Protocol 777 bot client load generator.

use std::env;
use std::time::Duration;
use tokio::net::TcpStream;
use tracing::{error, info};
use zenith_wire::{
    Handshake, HandshakeIntent, KeepAlive, LoginStart, PROTOCOL_VERSION, read_raw_packet,
    write_raw_packet,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = env::args().collect();
    let target = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:25565".to_string());
    let bot_count: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
    let stagger_ms: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(250);

    info!("=======================================================");
    info!("   ZENITH REAL TCP PROTOCOL 777 BOT SWARM HARNESS      ");
    info!(
        "   Target: {} | Bots: {} | Stagger: {}ms",
        target, bot_count, stagger_ms
    );
    info!("=======================================================");

    let mut handles = Vec::new();

    for id in 1..=bot_count {
        let target_clone = target.clone();
        tokio::time::sleep(Duration::from_millis(stagger_ms)).await;

        let handle = tokio::spawn(async move {
            if let Err(e) = run_single_bot(id, &target_clone).await {
                error!(bot_id = id, error = %e, "Bot connection failed");
            }
        });
        handles.push(handle);
    }

    for h in handles {
        let _ = h.await;
    }

    info!("All bots finished their transit cycles.");
    Ok(())
}

async fn run_single_bot(
    id: usize,
    target: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut stream = TcpStream::connect(target).await?;
    let bot_name = format!("Bot_{:02}", id);

    // 1. Handshake (Intent = Login)
    let hs = Handshake {
        protocol_version: PROTOCOL_VERSION,
        server_address: "127.0.0.1".into(),
        server_port: 25565,
        intent: HandshakeIntent::Login,
    };
    write_raw_packet(&mut stream, 0x00, &hs.encode()).await?;

    // 2. Login Start
    let start = LoginStart {
        name: bot_name.clone(),
        uuid: id as u128,
    };
    write_raw_packet(&mut stream, 0x00, &start.encode()).await?;

    // 3. Receive Login Success (0x02)
    let (success_id, _) = read_raw_packet(&mut stream, 2048).await?;
    if success_id != 0x02 {
        return Err(format!("Expected 0x02 Login Success, got 0x{success_id:02X}").into());
    }

    // 4. Send Login Acknowledged (0x03)
    write_raw_packet(&mut stream, 0x03, &[]).await?;

    // 5. Receive Finish Configuration (0x03)
    let (finish_cfg_id, _) = read_raw_packet(&mut stream, 2048).await?;
    if finish_cfg_id != 0x03 {
        return Err(format!("Expected 0x03 Finish Config, got 0x{finish_cfg_id:02X}").into());
    }

    // 6. Send Finish Configuration Acknowledgment (0x03)
    write_raw_packet(&mut stream, 0x03, &[]).await?;

    // 7. Receive Login (Play) (50) & Synchronize Position (73)
    let (p1, _) = read_raw_packet(&mut stream, 8192).await?;
    let (p2, _) = read_raw_packet(&mut stream, 8192).await?;
    info!(bot = %bot_name, p1 = p1, p2 = p2, "Bot entered Zenith virtual holding limbo");

    // 8. Main holding loop: answer keepalives and await Transfer packet (132 in Protocol 777)
    loop {
        let (packet_id, payload) = match read_raw_packet(&mut stream, 16384).await {
            Ok(p) => p,
            Err(_) => break, // Disconnected or transferred
        };

        match packet_id {
            45 => {
                // KeepAlive ping from server -> respond with KeepAlive pong (ID 28)
                if let Ok(ka) = KeepAlive::decode(&payload) {
                    let _ = write_raw_packet(&mut stream, 28, &ka.encode()).await;
                }
            }
            132 => {
                // 132 (0x84) Transfer packet received!
                info!(bot = %bot_name, ">>> RECEIVED 132 (0x84) TRANSFER PACKET! Transitioning to backend! <<<");
                break;
            }
            _ => {
                // Ignore display HUD packets (BossBar, Actionbar, Titles, Sound)
            }
        }
    }

    Ok(())
}
