//! Platform-specific battery read, isolated at the `Context`-build edge.
//!
//! `read_battery` is the only `#[cfg(target_os)]` surface; each arm delegates
//! to a pure parser (`parse_linux`/`parse_pmset`) that compiles under `test`
//! on any host, so both parsers are unit-tested on the Linux dev box even
//! though only one arm's reader compiles per platform.

use rustline_core::{Battery, BatteryState};

/// Read the host battery, or `None` if there is no battery, the platform is
/// unsupported, or the read failed. Called once at Context-build time.
pub fn read_battery() -> Option<Battery> {
    #[cfg(target_os = "linux")]
    {
        read_battery_linux()
    }
    #[cfg(target_os = "macos")]
    {
        read_battery_macos()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn read_battery_linux() -> Option<Battery> {
    let base = std::path::Path::new("/sys/class/power_supply");
    for entry in std::fs::read_dir(base).ok()? {
        let Ok(entry) = entry else { continue };
        let dir = entry.path();
        // Only real batteries (type == "Battery"), not Mains/AC adapters.
        let is_battery = std::fs::read_to_string(dir.join("type"))
            .map(|t| t.trim() == "Battery")
            .unwrap_or(false);
        if !is_battery {
            continue;
        }
        let (Ok(capacity), Ok(status)) = (
            std::fs::read_to_string(dir.join("capacity")),
            std::fs::read_to_string(dir.join("status")),
        ) else {
            continue;
        };
        return parse_linux(&capacity, &status);
    }
    None
}

#[cfg(target_os = "macos")]
fn read_battery_macos() -> Option<Battery> {
    let output = std::process::Command::new("pmset")
        .args(["-g", "batt"])
        .output()
        .ok()?;
    let stdout = String::from_utf8(output.stdout).ok()?;
    parse_pmset(&stdout)
}

/// Parse Linux sysfs `capacity` + `status` file contents into a `Battery`.
/// Non-numeric capacity → `None`; out-of-range capacity clamps to 100.
#[cfg(any(target_os = "linux", test))]
fn parse_linux(capacity: &str, status: &str) -> Option<Battery> {
    let percent = capacity.trim().parse::<u32>().ok()?.min(100) as u8;
    let state = match status.trim().to_ascii_lowercase().as_str() {
        "charging" => BatteryState::Charging,
        "discharging" => BatteryState::Discharging,
        // "Not charging" = plugged in, topped off; shown as Full.
        "full" | "not charging" => BatteryState::Full,
        _ => BatteryState::Unknown,
    };
    Some(Battery { percent, state })
}

/// Parse `pmset -g batt` stdout into a `Battery`. Reads the first line
/// containing a `%`, taking the digit run before `%` as the percentage and the
/// word after the first `;` as the state. No battery / no percent → `None`.
#[cfg(any(target_os = "macos", test))]
fn parse_pmset(output: &str) -> Option<Battery> {
    let line = output.lines().find(|l| l.contains('%'))?;
    let pct_end = line.find('%')?;
    let percent = line[..pct_end]
        .rsplit(|c: char| !c.is_ascii_digit())
        .next()
        .filter(|d| !d.is_empty())?
        .parse::<u32>()
        .ok()?
        .min(100) as u8;
    let state = line[pct_end + 1..]
        .split(';')
        .nth(1)
        .map(str::trim)
        .map(|s| match s.to_ascii_lowercase().as_str() {
            "charging" | "finishing charge" => BatteryState::Charging,
            "discharging" => BatteryState::Discharging,
            "charged" => BatteryState::Full,
            _ => BatteryState::Unknown,
        })
        .unwrap_or(BatteryState::Unknown);
    Some(Battery { percent, state })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustline_core::BatteryState::*;

    #[test]
    fn linux_parses_percent_and_state() {
        assert_eq!(parse_linux("73\n", "Discharging\n").unwrap().percent, 73);
        assert_eq!(parse_linux("73", "Discharging").unwrap().state, Discharging);
        assert_eq!(parse_linux("55", "Charging").unwrap().state, Charging);
        assert_eq!(parse_linux("100", "Full").unwrap().state, Full);
        // "Not charging" = plugged in and topped off -> Full for display.
        assert_eq!(parse_linux("100", "Not charging").unwrap().state, Full);
        // Unknown status word.
        assert_eq!(parse_linux("40", "Weird").unwrap().state, Unknown);
    }

    #[test]
    fn linux_clamps_and_rejects_garbage() {
        assert_eq!(parse_linux("150", "Full").unwrap().percent, 100); // clamp
        assert!(parse_linux("nope", "Full").is_none()); // non-numeric -> None
    }

    #[test]
    fn pmset_parses_discharging() {
        let out = "Now drawing from 'Battery Power'\n \
            -InternalBattery-0 (id=1234567)\t73%; discharging; 3:21 remaining present: true\n";
        let b = parse_pmset(out).unwrap();
        assert_eq!(b.percent, 73);
        assert_eq!(b.state, Discharging);
    }

    #[test]
    fn pmset_parses_charging_and_charged() {
        let charging = " -InternalBattery-0 (id=1)\t46%; charging; 1:12 remaining present: true\n";
        assert_eq!(parse_pmset(charging).unwrap().percent, 46);
        assert_eq!(parse_pmset(charging).unwrap().state, Charging);

        let charged = " -InternalBattery-0 (id=1)\t100%; charged; 0:00 remaining present: true\n";
        assert_eq!(parse_pmset(charged).unwrap().state, Full);
    }

    #[test]
    fn pmset_rejects_no_battery() {
        assert!(parse_pmset("Now drawing from 'AC Power'\nNo internal battery\n").is_none());
        assert!(parse_pmset("garbage with no percent sign").is_none());
    }

    #[test]
    fn read_battery_never_panics() {
        // Host-dependent value; only assert it does not panic and is in range.
        if let Some(b) = read_battery() {
            assert!(b.percent <= 100);
        }
    }
}
