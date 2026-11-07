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
        if let Ok(val) = std::env::var("ZENITH_OWNERS") {
            for name in val.split(',') {
                let trimmed = name.trim().to_lowercase();
                if !trimmed.is_empty() {
                    owners.insert(trimmed);
                }
            }
        }

        let mut staff = HashSet::new();
        if let Ok(val) = std::env::var("ZENITH_STAFF") {
            for name in val.split(',') {
                let trimmed = name.trim().to_lowercase();
                if !trimmed.is_empty() {
                    staff.insert(trimmed);
                }
            }
        }

        let mut vips = HashSet::new();
        if let Ok(val) = std::env::var("ZENITH_VIPS") {
            for name in val.split(',') {
                let trimmed = name.trim().to_lowercase();
                if !trimmed.is_empty() {
                    vips.insert(trimmed);
                }
            }
        }

        Self {
            owners,
            staff,
            vips,
        }
    }
}

impl GroupManager {
    /// Creates a new group manager with empty groups or loaded from environment variables.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks if any staff or owner exists in the system.
    #[must_use]
    pub fn has_any_staff(&self) -> bool {
        !self.owners.is_empty() || !self.staff.is_empty()
    }

    /// Checks if a player is in the Owners group.
    /// If no owners are configured yet, allows access for bootstrap setup.
    #[must_use]
    pub fn is_owner(&self, username: &str) -> bool {
        if self.owners.is_empty() {
            return true;
        }
        self.owners.contains(&username.to_lowercase())
    }

    /// Checks if a player has Staff privileges (Owner or Staff).
    /// If no staff or owners are configured yet, allows access for bootstrap setup.
    #[must_use]
    pub fn is_staff(&self, username: &str) -> bool {
        if !self.has_any_staff() {
            return true;
        }
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
        // By default, no hardcoded staff or owners from external servers
        assert!(mgr.owners().is_empty());
        assert!(mgr.staff_members().is_empty());
        assert!(mgr.vip_members().is_empty());
        assert_eq!(mgr.priority_of("random_player"), QueuePriority::Standard);
    }

    #[test]
    fn test_group_manager_add_remove() {
        let mut mgr = GroupManager::default();
        mgr.add("owner", "Axxio").unwrap();
        assert!(mgr.is_owner("axxio"));
        assert!(mgr.is_staff("axxio"));
        assert!(!mgr.is_staff("random_player"));
        assert_eq!(mgr.priority_of("axxio"), QueuePriority::Staff);

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
