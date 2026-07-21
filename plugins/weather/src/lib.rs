//! rustline `weather` plugin: shows a Nerd-Font condition icon + °F for a zip
//! code from wttr.in, fetched via the host's `rl_http_get_cached` (the host
//! owns the TTL cache: at most one live fetch per `refresh_secs`).
//!
//! The pure logic (icon map, formatting, JSON parse) is compiled and
//! unit-tested on the host; the Extism guest glue is wasm-only (see bottom).

use serde::Deserialize;

/// Map a WWO `weatherCode` to a Nerd-Font weather glyph. Unknown → `nf-weather-na`.
pub fn code_to_icon(code: &str) -> &'static str {
    match code {
        "113" => "\u{e30d}",                 // Sunny / Clear
        "116" => "\u{e302}",                 // Partly cloudy
        "119" | "122" => "\u{e312}",         // Cloudy / Overcast
        "143" | "248" | "260" => "\u{e313}", // Mist / Fog
        "176" | "263" | "266" | "281" | "284" | "293" | "296" | "299" | "302" | "305" | "308"
        | "311" | "314" | "353" | "356" | "359" => "\u{e318}", // Rain
        "200" | "386" | "389" | "392" | "395" => "\u{e31d}", // Thundery
        "179" | "182" | "185" | "227" | "230" | "317" | "320" | "323" | "326" | "329" | "332"
        | "335" | "338" | "350" | "362" | "365" | "368" | "371" | "374" | "377" => "\u{e31a}", // Snow / Sleet
        _ => "\u{e374}", // na (unknown)
    }
}

/// Substitute `{icon}`, `{temp_f}`, `{conditions}`, `{zip}` in `fmt`. Unknown
/// placeholders pass through untouched.
pub fn render_format(fmt: &str, icon: &str, temp_f: &str, conditions: &str, zip: &str) -> String {
    fmt.replace("{icon}", icon)
        .replace("{temp_f}", temp_f)
        .replace("{conditions}", conditions)
        .replace("{zip}", zip)
}

/// Extracted wttr.in current conditions.
pub struct Wttr {
    pub temp_f: String,
    pub code: String,
    pub desc: String,
}

#[derive(Deserialize)]
struct WttrJson {
    current_condition: Vec<CurrentCondition>,
}

#[derive(Deserialize)]
struct CurrentCondition {
    #[serde(rename = "temp_F")]
    temp_f: String,
    #[serde(rename = "weatherCode")]
    code: String,
    #[serde(rename = "weatherDesc", default)]
    desc: Vec<DescVal>,
}

#[derive(Deserialize)]
struct DescVal {
    value: String,
}

/// Parse a wttr.in `format=j1` body into the current conditions.
pub fn parse_wttr(json: &str) -> Option<Wttr> {
    let parsed: WttrJson = serde_json::from_str(json).ok()?;
    let cc = parsed.current_condition.into_iter().next()?;
    Some(Wttr {
        temp_f: cc.temp_f,
        code: cc.code,
        desc: cc
            .desc
            .into_iter()
            .next()
            .map(|d| d.value)
            .unwrap_or_default(),
    })
}

/// Pick the active weather format given whether this plugin is toggled and its
/// configured `alt_format` (mirrors the host's `active_format`).
pub fn select_weather_format<'a>(toggled: bool, format: &'a str, alt_format: &'a str) -> &'a str {
    if toggled && !alt_format.is_empty() {
        alt_format
    } else {
        format
    }
}

#[cfg(target_arch = "wasm32")]
mod guest {
    use super::*;
    use extism_pdk::*;
    use rustline_abi::Segment;
    use serde_json::Value;

    #[host_fn]
    extern "ExtismHost" {
        fn rl_http_get_cached(url: String, ttl_secs: String, now: String) -> String;
    }

    #[plugin_fn]
    pub fn name() -> FnResult<String> {
        Ok("weather".to_string())
    }

    #[plugin_fn]
    pub fn render(input: String) -> FnResult<Json<Vec<Segment>>> {
        let v: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
        let now = v["context"]["now"].as_str().unwrap_or_default().to_string();
        let cfg = &v["config"];
        let zip = cfg["zip"].as_str().unwrap_or("48183").to_string();
        let raw_format = cfg["format"].as_str().unwrap_or("{icon} {temp_f}°F");
        // `context.toggled` is a JSON array of names; this plugin is toggled
        // when it contains its own name, "weather".
        let toggled = v
            .get("context")
            .and_then(|c| c.get("toggled"))
            .and_then(|t| t.as_array())
            .is_some_and(|a| a.iter().any(|name| name.as_str() == Some("weather")));
        let alt_format = cfg["alt_format"].as_str().unwrap_or("");
        let format = select_weather_format(toggled, raw_format, alt_format);
        let refresh_secs = cfg["refresh_secs"].as_i64().unwrap_or(1800);
        let api_base = cfg["api_base"]
            .as_str()
            .unwrap_or("https://wttr.in")
            .to_string();

        // The host owns the TTL cache: fetch at most once per refresh_secs,
        // serving a fresh or last-good-stale body. Keyed by URL, so a zip
        // change is a different cache entry (no cross-zip leakage).
        let url = format!("{api_base}/{zip}?format=j1");
        let seg = unsafe { rl_http_get_cached(url, refresh_secs.to_string(), now) }
            .ok()
            .and_then(|raw| {
                let r: Value = serde_json::from_str(&raw).ok()?;
                if r["ok"].as_bool().unwrap_or(false) {
                    parse_wttr(r["body"].as_str().unwrap_or_default())
                } else {
                    None
                }
            })
            .map(|w| segment(format, &w.code, &w.temp_f, &w.desc, &zip))
            .unwrap_or_default();
        Ok(Json(seg))
    }

    fn segment(format: &str, code: &str, temp_f: &str, desc: &str, zip: &str) -> Vec<Segment> {
        let text = render_format(format, code_to_icon(code), temp_f, desc, zip);
        // one unstyled segment; the host assigns palette for left/right regions
        vec![Segment::new(text)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_maps_known_and_unknown_codes() {
        assert_eq!(code_to_icon("113"), "\u{e30d}"); // clear/sunny
        assert_eq!(code_to_icon("116"), "\u{e302}"); // partly cloudy
        assert_eq!(code_to_icon("296"), "\u{e318}"); // rain
        assert_eq!(code_to_icon("999"), "\u{e374}"); // unknown -> fallback (na)
    }

    #[test]
    fn format_substitutes_placeholders_and_passes_unknowns() {
        let out = render_format(
            "{icon} {temp_f}°F {conditions} @{zip}",
            "☀",
            "72",
            "Sunny",
            "48183",
        );
        assert_eq!(out, "☀ 72°F Sunny @48183");
        // unknown placeholder is left untouched
        assert_eq!(render_format("{bogus}", "i", "1", "c", "z"), "{bogus}");
    }

    #[test]
    fn parse_wttr_extracts_current_condition() {
        let j = r#"{"current_condition":[{"temp_F":"72","weatherCode":"113","weatherDesc":[{"value":"Sunny"}]}]}"#;
        let w = parse_wttr(j).unwrap();
        assert_eq!(w.temp_f, "72");
        assert_eq!(w.code, "113");
        assert_eq!(w.desc, "Sunny");
        assert!(parse_wttr("{}").is_none());
    }
}

#[cfg(test)]
mod toggle_tests {
    use super::select_weather_format;

    #[test]
    fn toggled_prefers_nonempty_alt() {
        assert_eq!(
            select_weather_format(true, "{icon} {temp_f}", "{icon} {temp_f}°F {city}"),
            "{icon} {temp_f}°F {city}"
        );
        assert_eq!(select_weather_format(false, "F", "A"), "F");
        assert_eq!(select_weather_format(true, "F", ""), "F");
    }
}
