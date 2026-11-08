//! CPU Core Pinning and Process Affinity Management for Zenith Benchmarks.
//!
//! Isolates the server daemon under test onto dedicated Performance Cores (P-cores)
//! while pinning the synthetic bot load generator onto Efficiency Cores (E-cores).
//! This prevents the benchmark load generator from starving the server of CPU cycles.
//!
//! 100% Safe Rust: Uses OS process supervision (`powershell ProcessorAffinity` on Windows,
//! `taskset` on Linux) to strictly comply with `#![forbid(unsafe_code)]`.

/// Core affinity configuration and topology helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreTopology {
    pub total_logical_cores: usize,
    pub p_cores_mask: usize,
    pub e_cores_mask: usize,
}

impl CoreTopology {
    /// Detects system CPU topology and builds masks for P-cores vs E-cores.
    pub fn detect() -> Self {
        let total = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8);

        // On hybrid architectures (e.g. Intel 12th/13th/14th Gen such as i5-12450H / i9-14900):
        // P-cores typically have 2 SMT threads each at lower IDs (0..P*2),
        // while E-cores are single-threaded at higher IDs.
        // For an 8+4 or 8+8 system, logical 0-7 are typical primary P-cores.
        let p_core_count = if total >= 16 {
            16
        } else if total >= 12 {
            8
        } else if total >= 8 {
            6
        } else {
            total.max(2) / 2
        };

        let mut p_mask: usize = 0;
        for i in 0..p_core_count.min(total) {
            p_mask |= 1 << i;
        }

        let mut e_mask: usize = 0;
        for i in p_core_count..total {
            e_mask |= 1 << i;
        }
        if e_mask == 0 {
            e_mask = p_mask; // Fallback if no separate E-cores exist
        }

        Self {
            total_logical_cores: total,
            p_cores_mask: p_mask,
            e_cores_mask: e_mask,
        }
    }
}

/// Parses an affinity string specification into a bitmask.
///
/// Supported formats:
/// - `"0-3"`: Range of logical core IDs (0, 1, 2, 3)
/// - `"0,1,2,3"`: Comma-separated list of core IDs
/// - `"p-cores"` or `"pcores"`: Primary Performance Cores
/// - `"e-cores"` or `"ecores"`: Secondary Efficiency Cores
/// - `"all"`: All available logical cores
pub fn parse_affinity_mask(arg: &str) -> Result<usize, String> {
    let topo = CoreTopology::detect();
    let lower = arg.trim().to_ascii_lowercase();

    match lower.as_str() {
        "p-cores" | "pcores" | "p" => Ok(topo.p_cores_mask),
        "e-cores" | "ecores" | "e" => Ok(topo.e_cores_mask),
        "all" => {
            let mut mask: usize = 0;
            for i in 0..topo.total_logical_cores.min(usize::BITS as usize) {
                mask |= 1 << i;
            }
            Ok(mask)
        }
        _ if lower.contains('-') => {
            let parts: Vec<&str> = lower.split('-').collect();
            if parts.len() != 2 {
                return Err(format!("Invalid core range: {arg}"));
            }
            let start: usize = parts[0]
                .trim()
                .parse()
                .map_err(|e| format!("Invalid range start: {e}"))?;
            let end: usize = parts[1]
                .trim()
                .parse()
                .map_err(|e| format!("Invalid range end: {e}"))?;
            if start > end || end >= usize::BITS as usize {
                return Err(format!("Invalid range boundaries: {start}-{end}"));
            }
            let mut mask: usize = 0;
            for i in start..=end {
                mask |= 1 << i;
            }
            Ok(mask)
        }
        _ if lower.contains(',') => {
            let mut mask: usize = 0;
            for item in lower.split(',') {
                let id: usize = item
                    .trim()
                    .parse()
                    .map_err(|e| format!("Invalid core ID '{item}': {e}"))?;
                if id >= usize::BITS as usize {
                    return Err(format!("Core ID {id} exceeds maximum supported bits"));
                }
                mask |= 1 << id;
            }
            Ok(mask)
        }
        _ => {
            // Single core ID
            let id: usize = lower
                .parse()
                .map_err(|e| format!("Invalid core specifier '{arg}': {e}"))?;
            if id >= usize::BITS as usize {
                return Err(format!("Core ID {id} exceeds maximum supported bits"));
            }
            Ok(1 << id)
        }
    }
}

/// Applies CPU core affinity mask to the current calling process safely without unsafe code.
pub fn apply_process_affinity(mask: usize) -> Result<(), String> {
    let pid = std::process::id();

    #[cfg(target_os = "windows")]
    {
        let ps_cmd = format!("(Get-Process -Id {pid}).ProcessorAffinity = {mask}");
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
            .output()
            .map_err(|e| format!("Failed to execute powershell ProcessorAffinity: {e}"))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(format!("ProcessorAffinity command failed: {err}"));
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        let mask_hex = format!("{:x}", mask);
        let output = std::process::Command::new("taskset")
            .args(["-p", &mask_hex, &pid.to_string()])
            .output()
            .map_err(|e| format!("Failed to spawn taskset for affinity: {e}"))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(format!("taskset failed: {err}"));
        }
        Ok(())
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = mask;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_range_affinity() {
        let mask = parse_affinity_mask("0-3").unwrap();
        assert_eq!(mask, 0b1111);

        let mask2 = parse_affinity_mask("4-7").unwrap();
        assert_eq!(mask2, 0b1111_0000);
    }

    #[test]
    fn test_parse_comma_affinity() {
        let mask = parse_affinity_mask("0,2,4").unwrap();
        assert_eq!(mask, (1 << 0) | (1 << 2) | (1 << 4));
    }

    #[test]
    fn test_parse_keywords() {
        let p_mask = parse_affinity_mask("p-cores").unwrap();
        assert!(p_mask > 0);

        let e_mask = parse_affinity_mask("e-cores").unwrap();
        assert!(e_mask > 0);

        let all_mask = parse_affinity_mask("all").unwrap();
        assert!(all_mask > 0);
    }
}
