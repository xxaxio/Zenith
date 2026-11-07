//! Clientbound Commands (packet 0x10 / 16) Brigadier command tree generation.

use horizon_protocol::buf::{Codec, Writer};
use horizon_protocol::packets::play::{CommandNode, Commands};

/// Builds the Zenith command tree packet payload for Play state (Packet ID 16).
#[must_use]
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

    // /group <list | info | create | delete | setweight | setstaff | add | remove>
    {
        let group_idx = nodes.len() as i32;
        nodes[0].children.push(group_idx);

        // Placeholder for group node (index group_idx)
        nodes.push(CommandNode {
            kind: 1,
            executable: false,
            restricted: false,
            children: Vec::new(),
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

        // /group info <group>
        let info_idx = nodes.len() as i32;
        let info_arg_idx = info_idx + 1;
        nodes.push(CommandNode {
            kind: 1,
            executable: false,
            restricted: false,
            children: vec![info_arg_idx],
            redirect: None,
            name: "info".to_string(),
            parser: None,
            suggestions: None,
        });
        nodes[group_idx as usize].children.push(info_idx);

        nodes.push(CommandNode {
            kind: 2,
            executable: true,
            restricted: false,
            children: Vec::new(),
            redirect: None,
            name: "group".to_string(),
            parser: Some((5, vec![0])),
            suggestions: None,
        });

        // /group create <name> <weight>
        let create_idx = nodes.len() as i32;
        let create_name_idx = create_idx + 1;
        let create_weight_idx = create_idx + 2;
        nodes.push(CommandNode {
            kind: 1,
            executable: false,
            restricted: false,
            children: vec![create_name_idx],
            redirect: None,
            name: "create".to_string(),
            parser: None,
            suggestions: None,
        });
        nodes[group_idx as usize].children.push(create_idx);

        nodes.push(CommandNode {
            kind: 2,
            executable: false,
            restricted: false,
            children: vec![create_weight_idx],
            redirect: None,
            name: "name".to_string(),
            parser: Some((5, vec![0])),
            suggestions: None,
        });

        nodes.push(CommandNode {
            kind: 2,
            executable: true,
            restricted: false,
            children: Vec::new(),
            redirect: None,
            name: "weight".to_string(),
            parser: Some((5, vec![0])),
            suggestions: None,
        });

        // /group delete <group>
        let del_idx = nodes.len() as i32;
        let del_arg_idx = del_idx + 1;
        nodes.push(CommandNode {
            kind: 1,
            executable: false,
            restricted: false,
            children: vec![del_arg_idx],
            redirect: None,
            name: "delete".to_string(),
            parser: None,
            suggestions: None,
        });
        nodes[group_idx as usize].children.push(del_idx);

        nodes.push(CommandNode {
            kind: 2,
            executable: true,
            restricted: false,
            children: Vec::new(),
            redirect: None,
            name: "group".to_string(),
            parser: Some((5, vec![0])),
            suggestions: None,
        });

        // /group setweight <group> <weight>
        let sw_idx = nodes.len() as i32;
        let sw_grp_idx = sw_idx + 1;
        let sw_val_idx = sw_idx + 2;
        nodes.push(CommandNode {
            kind: 1,
            executable: false,
            restricted: false,
            children: vec![sw_grp_idx],
            redirect: None,
            name: "setweight".to_string(),
            parser: None,
            suggestions: None,
        });
        nodes[group_idx as usize].children.push(sw_idx);

        nodes.push(CommandNode {
            kind: 2,
            executable: false,
            restricted: false,
            children: vec![sw_val_idx],
            redirect: None,
            name: "group".to_string(),
            parser: Some((5, vec![0])),
            suggestions: None,
        });

        nodes.push(CommandNode {
            kind: 2,
            executable: true,
            restricted: false,
            children: Vec::new(),
            redirect: None,
            name: "weight".to_string(),
            parser: Some((5, vec![0])),
            suggestions: None,
        });

        // /group add <group> <player>
        let add_idx = nodes.len() as i32;
        let add_grp_idx = add_idx + 1;
        let add_plr_idx = add_idx + 2;
        nodes.push(CommandNode {
            kind: 1,
            executable: false,
            restricted: false,
            children: vec![add_grp_idx],
            redirect: None,
            name: "add".to_string(),
            parser: None,
            suggestions: None,
        });
        nodes[group_idx as usize].children.push(add_idx);

        nodes.push(CommandNode {
            kind: 2,
            executable: false,
            restricted: false,
            children: vec![add_plr_idx],
            redirect: None,
            name: "group".to_string(),
            parser: Some((5, vec![0])),
            suggestions: None,
        });

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

        // /group remove <group> <player>
        let rem_idx = nodes.len() as i32;
        let rem_grp_idx = rem_idx + 1;
        let rem_plr_idx = rem_idx + 2;
        nodes.push(CommandNode {
            kind: 1,
            executable: false,
            restricted: false,
            children: vec![rem_grp_idx],
            redirect: None,
            name: "remove".to_string(),
            parser: None,
            suggestions: None,
        });
        nodes[group_idx as usize].children.push(rem_idx);

        nodes.push(CommandNode {
            kind: 2,
            executable: false,
            restricted: false,
            children: vec![rem_plr_idx],
            redirect: None,
            name: "group".to_string(),
            parser: Some((5, vec![0])),
            suggestions: None,
        });

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

    // /broadcast <message> and /bc <message>
    for bc_alias in ["broadcast", "bc"] {
        let bc_idx = nodes.len() as i32;
        let msg_arg_idx = bc_idx + 1;
        nodes.push(CommandNode {
            kind: 1,
            executable: false,
            restricted: false,
            children: vec![msg_arg_idx],
            redirect: None,
            name: bc_alias.to_string(),
            parser: None,
            suggestions: None,
        });
        nodes[0].children.push(bc_idx);

        nodes.push(CommandNode {
            kind: 2,
            executable: true,
            restricted: false,
            children: Vec::new(),
            redirect: None,
            name: "message".to_string(),
            parser: Some((5, vec![2])), // 2 = greedy phrase
            suggestions: None,
        });
    }

    // /alert <message>
    {
        let alert_idx = nodes.len() as i32;
        let msg_arg_idx = alert_idx + 1;
        nodes.push(CommandNode {
            kind: 1,
            executable: false,
            restricted: false,
            children: vec![msg_arg_idx],
            redirect: None,
            name: "alert".to_string(),
            parser: None,
            suggestions: None,
        });
        nodes[0].children.push(alert_idx);

        nodes.push(CommandNode {
            kind: 2,
            executable: true,
            restricted: false,
            children: Vec::new(),
            redirect: None,
            name: "message".to_string(),
            parser: Some((5, vec![2])), // 2 = greedy phrase
            suggestions: None,
        });
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
