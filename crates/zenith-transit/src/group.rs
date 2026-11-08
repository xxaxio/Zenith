//! LuckPerms-style configurable dynamic groups with persistent TOML storage.

use crate::priority::QueuePriority;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// A configurable group with a unique name, priority weight, staff permission, and members.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Group {
    /// Unique group identifier (e.g. "admin", "vip", "default")
    pub name: String,
    /// Priority weight in the transit queue (higher weight = prioritized first)
    #[serde(default)]
    pub weight: u32,
    /// Whether members of this group have Zenith admin/management permissions
    #[serde(default)]
    pub is_staff: bool,
    /// Registered player usernames (case-insensitive)
    #[serde(default)]
    pub members: HashSet<String>,
}

impl Group {
    /// Creates a new group.
    #[must_use]
    pub fn new(name: impl Into<String>, weight: u32, is_staff: bool) -> Self {
        Self {
            name: name.into(),
            weight,
            is_staff,
            members: HashSet::new(),
        }
    }
}

/// Serialized format for `groups.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ConfigFile {
    #[serde(default)]
    pub groups: Vec<GroupConfigEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GroupConfigEntry {
    pub name: String,
    #[serde(default)]
    pub weight: u32,
    #[serde(default)]
    pub is_staff: bool,
    #[serde(default)]
    pub members: Vec<String>,
}

/// Manages dynamic groups, persistence to `groups.toml`, and priority resolution.
#[derive(Debug, Clone)]
pub struct GroupManager {
    groups: HashMap<String, Group>,
    config_path: Option<PathBuf>,
}

impl Default for GroupManager {
    fn default() -> Self {
        Self::new(None)
    }
}

impl GroupManager {
    /// Creates a new GroupManager. If `config_path` is given and exists, loads from file.
    #[must_use]
    pub fn new(config_path: Option<PathBuf>) -> Self {
        let mut mgr = Self {
            groups: HashMap::new(),
            config_path,
        };

        if let Some(ref path) = mgr.config_path.clone() {
            if path.exists() {
                if let Err(e) = mgr.load_from_path(path) {
                    tracing::warn!(error = %e, path = %path.display(), "Failed to load groups.toml, using defaults");
                    mgr.ensure_default_groups();
                }
            } else {
                mgr.ensure_default_groups();
                let _ = mgr.save();
            }
        } else {
            mgr.ensure_default_groups();
        }

        mgr
    }

    /// Ensures that at least a "default" group exists.
    fn ensure_default_groups(&mut self) {
        if !self.groups.contains_key("default") {
            self.groups
                .insert("default".to_string(), Group::new("default", 0, false));
        }
    }

    /// Loads groups from a TOML file.
    pub fn load_from_path(&mut self, path: &Path) -> Result<(), String> {
        let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let parsed: ConfigFile = toml::from_str(&content).map_err(|e| e.to_string())?;

        self.groups.clear();
        for entry in parsed.groups {
            let key = entry.name.to_lowercase();
            let mut members = HashSet::new();
            for m in entry.members {
                members.insert(m.to_lowercase());
            }
            self.groups.insert(
                key,
                Group {
                    name: entry.name,
                    weight: entry.weight,
                    is_staff: entry.is_staff,
                    members,
                },
            );
        }

        self.ensure_default_groups();
        Ok(())
    }

    /// Saves groups to the configured TOML file.
    pub fn save(&self) -> Result<(), String> {
        let Some(ref path) = self.config_path else {
            return Ok(());
        };

        let mut entries = Vec::new();
        let mut sorted_groups: Vec<_> = self.groups.values().collect();
        sorted_groups.sort_by_key(|a| std::cmp::Reverse(a.weight));

        for g in sorted_groups {
            let mut mem_list: Vec<_> = g.members.iter().cloned().collect();
            mem_list.sort();
            entries.push(GroupConfigEntry {
                name: g.name.clone(),
                weight: g.weight,
                is_staff: g.is_staff,
                members: mem_list,
            });
        }

        let cfg = ConfigFile { groups: entries };
        let toml_str = toml::to_string_pretty(&cfg).map_err(|e| e.to_string())?;

        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(path, toml_str).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Checks if a group exists.
    #[must_use]
    pub fn has_group(&self, name: &str) -> bool {
        self.groups.contains_key(&name.to_lowercase())
    }

    /// Retrieves a group by name (case-insensitive).
    #[must_use]
    pub fn get_group(&self, name: &str) -> Option<&Group> {
        self.groups.get(&name.to_lowercase())
    }

    /// Returns all groups sorted by weight descending.
    #[must_use]
    pub fn all_groups(&self) -> Vec<&Group> {
        let mut list: Vec<&Group> = self.groups.values().collect();
        list.sort_by(|a, b| b.weight.cmp(&a.weight).then_with(|| a.name.cmp(&b.name)));
        list
    }

    /// Creates a new group.
    pub fn create_group(
        &mut self,
        name: &str,
        weight: u32,
        is_staff: bool,
    ) -> Result<(), &'static str> {
        let lower = name.trim().to_lowercase();
        if lower.is_empty() {
            return Err("Group name cannot be empty");
        }
        if self.groups.contains_key(&lower) {
            return Err("A group with this name already exists");
        }

        self.groups
            .insert(lower.clone(), Group::new(name.trim(), weight, is_staff));
        let _ = self.save();
        Ok(())
    }

    /// Deletes a group. Cannot delete the default group.
    pub fn delete_group(&mut self, name: &str) -> Result<(), &'static str> {
        let lower = name.trim().to_lowercase();
        if lower == "default" {
            return Err("Cannot delete the 'default' group");
        }
        if self.groups.remove(&lower).is_none() {
            return Err("Group not found");
        }
        let _ = self.save();
        Ok(())
    }

    /// Updates the weight of a group.
    pub fn set_weight(&mut self, name: &str, weight: u32) -> Result<(), &'static str> {
        let lower = name.trim().to_lowercase();
        let group = self.groups.get_mut(&lower).ok_or("Group not found")?;
        group.weight = weight;
        let _ = self.save();
        Ok(())
    }

    /// Updates the staff permission of a group.
    pub fn set_staff(&mut self, name: &str, is_staff: bool) -> Result<(), &'static str> {
        let lower = name.trim().to_lowercase();
        let group = self.groups.get_mut(&lower).ok_or("Group not found")?;
        group.is_staff = is_staff;
        let _ = self.save();
        Ok(())
    }

    /// Adds a player to a group.
    pub fn add_member(&mut self, group_name: &str, username: &str) -> Result<(), &'static str> {
        let lower_grp = group_name.trim().to_lowercase();
        let lower_user = username.trim().to_lowercase();
        if lower_user.is_empty() {
            return Err("Player username cannot be empty");
        }

        let group = self.groups.get_mut(&lower_grp).ok_or("Group not found")?;
        group.members.insert(lower_user);
        let _ = self.save();
        Ok(())
    }

    /// Removes a player from a group.
    pub fn remove_member(&mut self, group_name: &str, username: &str) -> Result<(), &'static str> {
        let lower_grp = group_name.trim().to_lowercase();
        let lower_user = username.trim().to_lowercase();

        let group = self.groups.get_mut(&lower_grp).ok_or("Group not found")?;
        if !group.members.remove(&lower_user) {
            return Err("Player is not a member of this group");
        }
        let _ = self.save();
        Ok(())
    }

    /// Returns the highest weight for a player across all groups they belong to.
    #[must_use]
    pub fn weight_of(&self, username: &str) -> u32 {
        let lower = username.to_lowercase();
        let mut max_weight = 0;
        for g in self.groups.values() {
            if g.members.contains(&lower) && g.weight > max_weight {
                max_weight = g.weight;
            }
        }
        max_weight
    }

    /// Returns the mapped QueuePriority for a player based on their highest group weight.
    #[must_use]
    pub fn priority_of(&self, username: &str) -> QueuePriority {
        QueuePriority::from(self.weight_of(username))
    }

    /// Checks if any player is designated as staff in any group.
    #[must_use]
    pub fn has_any_active_staff(&self) -> bool {
        self.groups
            .values()
            .any(|g| g.is_staff && !g.members.is_empty())
    }

    /// Checks if a player has Staff privileges.
    /// Strictly fails closed: returns true only if the player is an explicitly registered
    /// member of an active staff group.
    #[must_use]
    pub fn is_staff(&self, username: &str) -> bool {
        let lower = username.to_lowercase();
        self.groups
            .values()
            .any(|g| g.is_staff && g.members.contains(&lower))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dynamic_group_lifecycle() {
        let mut mgr = GroupManager::default();

        // Default group exists
        assert!(mgr.has_group("default"));
        assert_eq!(mgr.priority_of("anyone"), QueuePriority::Standard);

        // Create groups
        mgr.create_group("vip", 50, false).unwrap();
        mgr.create_group("admin", 1000, true).unwrap();

        assert!(mgr.has_group("VIP"));
        assert!(mgr.has_group("Admin"));

        // Add admin member first so active staff is established
        mgr.add_member("admin", "Axxio").unwrap();
        assert_eq!(mgr.weight_of("axxio"), 1000);
        assert!(mgr.is_staff("axxio"));

        // Add regular VIP member
        mgr.add_member("vip", "Joueur1").unwrap();
        assert_eq!(mgr.weight_of("joueur1"), 50);
        assert_eq!(mgr.priority_of("joueur1"), QueuePriority::Custom(50));
        assert!(!mgr.is_staff("joueur1"));

        // Staff check when staff is established
        assert!(!mgr.is_staff("random_person"));

        // Multi-group highest weight resolution
        mgr.add_member("admin", "Joueur1").unwrap();
        assert_eq!(mgr.weight_of("joueur1"), 1000);
        assert!(mgr.is_staff("joueur1"));

        // Remove from admin, falls back to VIP
        mgr.remove_member("admin", "joueur1").unwrap();
        assert_eq!(mgr.weight_of("joueur1"), 50);
        assert!(!mgr.is_staff("joueur1"));
    }

    #[test]
    fn test_toml_persistence() {
        let temp_dir = std::env::temp_dir().join(format!(
            "zenith_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config_path = temp_dir.join("groups.toml");

        {
            let mut mgr = GroupManager::new(Some(config_path.clone()));
            mgr.create_group("moderateur", 500, true).unwrap();
            mgr.add_member("moderateur", "ModoTest").unwrap();
        }

        // Reload from disk
        {
            let mgr = GroupManager::new(Some(config_path.clone()));
            assert!(mgr.has_group("moderateur"));
            let mod_group = mgr.get_group("moderateur").unwrap();
            assert_eq!(mod_group.weight, 500);
            assert!(mod_group.is_staff);
            assert!(mod_group.members.contains("modotest"));
            assert_eq!(mgr.weight_of("modotest"), 500);
        }

        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_is_staff_fails_closed_when_empty() {
        let mgr = GroupManager::default();
        // With default group only and no staff registered, all queries must return false
        assert!(!mgr.has_any_active_staff());
        assert!(!mgr.is_staff("AdminUser"));
        assert!(!mgr.is_staff("random_user"));
        assert!(!mgr.is_staff("admin"));
    }
}
