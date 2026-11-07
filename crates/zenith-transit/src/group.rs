//! Group and permission management for priority tiers and admin controls.

use crate::priority::QueuePriority;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Manages player group memberships (Owners, Staff, VIPs) and maps them to priority tiers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupManager {
    owners: HashSet<String>,
    staff: HashSet<String>,
    vips: HashSet<String>,
}

impl Default for GroupManager {
    fn default() -> Self {
        let mut owners = HashSet::new();
        owners.insert("axxio_hc".to_string());
        owners.insert("yoyo1406".to_string());

        let mut staff = HashSet::new();
        for name in &[
            "axxio_hc",
            "yoyo1406",
            "tino_lino",
            "khaelys_",
            "ghostedge92",
            "alexis1pape",
            "redsavantmc",
            "bisounoursl",
        ] {
            staff.insert(name.to_string());
        }

        Self {
            owners,
            staff,
            vips: HashSet::new(),
        }
    }
}

impl GroupManager {
    /// Creates a new group manager with default owners and staff.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks if a player is in the Owners group.
    #[must_use]
    pub fn is_owner(&self, username: &str) -> bool {
        self.owners.contains(&username.to_lowercase())
    }

    /// Checks if a player has Staff privileges (Owner or Staff).
    #[must_use]
    pub fn is_staff(&self, username: &str) -> bool {
        let lower = username.to_lowercase();
        self.owners.contains(&lower) || self.staff.contains(&lower)
    }

    /// Checks if a player is in the VIP group.
    #[must_use]
    pub fn is_vip(&self, username: &str) -> bool {
        self.vips.contains(&username.to_lowercase())
    }

    /// Returns the assigned QueuePriority for a given player username.
    #[must_use]
    pub fn priority_of(&self, username: &str) -> QueuePriority {
        let lower = username.to_lowercase();
        if self.owners.contains(&lower) || self.staff.contains(&lower) {
            QueuePriority::Staff
        } else if self.vips.contains(&lower) {
            QueuePriority::Vip
        } else {
            QueuePriority::Standard
        }
    }

    /// Adds a player to a group (`owner`, `staff`, `vip`).
    pub fn add(&mut self, group: &str, username: &str) -> Result<(), &'static str> {
        let lower = username.to_lowercase();
        match group.to_lowercase().as_str() {
            "owner" | "owners" => {
                self.owners.insert(lower.clone());
                self.staff.insert(lower);
                Ok(())
            }
            "staff" => {
                self.staff.insert(lower);
                Ok(())
            }
            "vip" | "vips" => {
                self.vips.insert(lower);
                Ok(())
            }
            _ => Err("Groupe invalide (groupes: owner, staff, vip)"),
        }
    }

    /// Removes a player from a group (`owner`, `staff`, `vip`).
    pub fn remove(&mut self, group: &str, username: &str) -> Result<(), &'static str> {
        let lower = username.to_lowercase();
        match group.to_lowercase().as_str() {
            "owner" | "owners" => {
                self.owners.remove(&lower);
                Ok(())
            }
            "staff" => {
                self.staff.remove(&lower);
                Ok(())
            }
            "vip" | "vips" => {
                self.vips.remove(&lower);
                Ok(())
            }
            _ => Err("Groupe invalide (groupes: owner, staff, vip)"),
        }
    }

    /// Lists members of a specific group.
    #[must_use]
    pub fn list(&self, group: &str) -> Option<Vec<String>> {
        match group.to_lowercase().as_str() {
            "owner" | "owners" => {
                let mut v: Vec<_> = self.owners.iter().cloned().collect();
                v.sort();
                Some(v)
            }
            "staff" => {
                let mut v: Vec<_> = self.staff.iter().cloned().collect();
                v.sort();
                Some(v)
            }
            "vip" | "vips" => {
                let mut v: Vec<_> = self.vips.iter().cloned().collect();
                v.sort();
                Some(v)
            }
            _ => None,
        }
    }

    /// Returns all registered owners.
    #[must_use]
    pub fn owners(&self) -> Vec<String> {
        let mut v: Vec<_> = self.owners.iter().cloned().collect();
        v.sort();
        v
    }

    /// Returns all registered staff members.
    #[must_use]
    pub fn staff_members(&self) -> Vec<String> {
        let mut v: Vec<_> = self.staff.iter().cloned().collect();
        v.sort();
        v
    }

    /// Returns all registered VIP members.
    #[must_use]
    pub fn vip_members(&self) -> Vec<String> {
        let mut v: Vec<_> = self.vips.iter().cloned().collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_manager_defaults() {
        let mgr = GroupManager::default();
        assert!(mgr.is_owner("axxio_hc"));
        assert!(mgr.is_owner("AXXIO_HC"));
        assert!(mgr.is_staff("axxio_hc"));
        assert!(mgr.is_staff("tino_lino"));
        assert!(!mgr.is_staff("random_player"));
        assert_eq!(mgr.priority_of("axxio_hc"), QueuePriority::Staff);
        assert_eq!(mgr.priority_of("random_player"), QueuePriority::Standard);
    }

    #[test]
    fn test_group_manager_add_remove() {
        let mut mgr = GroupManager::default();
        mgr.add("vip", "TestPlayer").unwrap();
        assert!(mgr.is_vip("testplayer"));
        assert_eq!(mgr.priority_of("testplayer"), QueuePriority::Vip);

        mgr.remove("vip", "testplayer").unwrap();
        assert!(!mgr.is_vip("testplayer"));
        assert_eq!(mgr.priority_of("testplayer"), QueuePriority::Standard);

        mgr.add("staff", "Moderator1").unwrap();
        assert!(mgr.is_staff("moderator1"));
        assert_eq!(mgr.priority_of("moderator1"), QueuePriority::Staff);
    }
}
