//! Clientbound Commands (packet 0x10 / 16) Brigadier command tree generation.

use horizon_protocol::buf::{Codec, Writer};
use horizon_protocol::packets::play::{CommandNode, Commands};

/// Builds the Zenith command tree packet payload for Play state (Packet ID 16).
pub fn build_commands_payload() -> Vec<u8> {
    let mut nodes = Vec::new();

    // Node 0: Root node
    nodes.push(CommandNode {
        kind: 0,
        executable: false,
        restricted: false,
        children: Vec::new(),
        redirect: None,
        name: String::new(),
        parser: None,
        suggestions: None,
    });

    // Simple literal commands (executable directly without required arguments)
    let simple_cmds = [
        "transfer",
        "join",
        "queue",
        "queueinfo",
        "info",
        "status",
        "ping",
        "help",
        "spawn",
    ];

    for name in simple_cmds {
        let idx = nodes.len() as i32;
        nodes.push(CommandNode {
            kind: 1, // literal
            executable: true,
            restricted: false,
            children: Vec::new(),
            redirect: None,
            name: name.to_string(),
            parser: None,
            suggestions: None,
        });
        nodes[0].children.push(idx);
    }

    // /queueskip <player>
    {
        let skip_idx = nodes.len() as i32;
        let player_arg_idx = skip_idx + 1;

        // /queueskip literal
        nodes.push(CommandNode {
            kind: 1,
            executable: false,
            restricted: false,
            children: vec![player_arg_idx],
            redirect: None,
            name: "queueskip".to_string(),
            parser: None,
            suggestions: None,
        });
        nodes[0].children.push(skip_idx);

        // <player> argument (brigadier:string single word)
        nodes.push(CommandNode {
            kind: 2, // argument
            executable: true,
            restricted: false,
            children: Vec::new(),
            redirect: None,
            name: "player".to_string(),
            parser: Some((5, vec![0])), // 5 = brigadier:string, 0 = single word
            suggestions: None,
        });
    }

    // /group <list | add | remove> ...
    {
        let group_idx = nodes.len() as i32;
        nodes[0].children.push(group_idx);

        // Placeholder for group node (index group_idx)
        nodes.push(CommandNode {
            kind: 1,
            executable: false,
            restricted: false,
            children: Vec::new(), // will fill with list, add, remove
            redirect: None,
            name: "group".to_string(),
            parser: None,
            suggestions: None,
        });

        // /group list
        let list_idx = nodes.len() as i32;
        nodes.push(CommandNode {
            kind: 1,
            executable: true,
            restricted: false,
            children: Vec::new(),
            redirect: None,
            name: "list".to_string(),
            parser: None,
            suggestions: None,
        });
        nodes[group_idx as usize].children.push(list_idx);

        // /group add <owner|staff|vip> <player>
        let add_idx = nodes.len() as i32;
        nodes.push(CommandNode {
            kind: 1,
            executable: false,
            restricted: false,
            children: Vec::new(),
            redirect: None,
            name: "add".to_string(),
            parser: None,
            suggestions: None,
        });
        nodes[group_idx as usize].children.push(add_idx);

        for role in ["owner", "staff", "vip"] {
            let role_idx = nodes.len() as i32;
            let player_idx = role_idx + 1;

            nodes.push(CommandNode {
                kind: 1,
                executable: false,
                restricted: false,
                children: vec![player_idx],
                redirect: None,
                name: role.to_string(),
                parser: None,
                suggestions: None,
            });
            nodes[add_idx as usize].children.push(role_idx);

            nodes.push(CommandNode {
                kind: 2,
                executable: true,
                restricted: false,
                children: Vec::new(),
                redirect: None,
                name: "player".to_string(),
                parser: Some((5, vec![0])),
                suggestions: None,
            });
        }

        // /group remove <owner|staff|vip> <player>
        let remove_idx = nodes.len() as i32;
        nodes.push(CommandNode {
            kind: 1,
            executable: false,
            restricted: false,
            children: Vec::new(),
            redirect: None,
            name: "remove".to_string(),
            parser: None,
            suggestions: None,
        });
        nodes[group_idx as usize].children.push(remove_idx);

        for role in ["owner", "staff", "vip"] {
            let role_idx = nodes.len() as i32;
            let player_idx = role_idx + 1;

            nodes.push(CommandNode {
                kind: 1,
                executable: false,
                restricted: false,
                children: vec![player_idx],
                redirect: None,
                name: role.to_string(),
                parser: None,
                suggestions: None,
            });
            nodes[remove_idx as usize].children.push(role_idx);

            nodes.push(CommandNode {
                kind: 2,
                executable: true,
                restricted: false,
                children: Vec::new(),
                redirect: None,
                name: "player".to_string(),
                parser: Some((5, vec![0])),
                suggestions: None,
            });
        }
    }

    let commands = Commands { nodes, root: 0 };
    let mut writer = Writer::new();
    Commands::write(&commands, &mut writer);
    writer.data
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_protocol::buf::Reader;

    #[test]
    fn test_commands_packet_roundtrip() {
        let payload = build_commands_payload();
        assert!(!payload.is_empty());
        let mut reader = Reader::new(&payload);
        let decoded = Commands::read(&mut reader).expect("Failed to decode commands tree");
        assert_eq!(decoded.root, 0);
        assert!(decoded.nodes.len() > 10);
    }
}
