//! rustline `weather` plugin: shows a Nerd-Font condition icon + °F for a zip
//! code from wttr.in, cached to its own state dir (≤ 1 fetch / refresh_secs).
//!
//! The pure logic (icon map, formatting, freshness, JSON parse) is compiled and
//! unit-tested on the host; the Extism guest glue is wasm-only (see bottom).

use chrono::DateTime;
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

/// A cache is fresh iff it is for the same zip and was fetched within
/// `refresh_secs` of `now`. Unparseable timestamps → not fresh (forces refetch).
pub fn is_fresh(
    now_rfc3339: &str,
    fetched_rfc3339: &str,
    refresh_secs: i64,
    cache_zip: &str,
    want_zip: &str,
) -> bool {
    if cache_zip != want_zip {
        return false;
    }
    let (Ok(now), Ok(fetched)) = (
        DateTime::parse_from_rfc3339(now_rfc3339),
        DateTime::parse_from_rfc3339(fetched_rfc3339),
    ) else {
        return false;
    };
    let age = now.timestamp() - fetched.timestamp();
    (0..refresh_secs).contains(&age)
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

#[cfg(target_arch = "wasm32")]
mod guest {
    use super::*;
    use extism_pdk::*;
    use serde_json::Value;

    #[host_fn]
    extern "ExtismHost" {
        fn rl_http_get(url: String) -> String;
        fn rl_state_read(relpath: String) -> String;
        fn rl_state_write(relpath: String, contents: String) -> String;
    }

    #[plugin_fn]
    pub fn name() -> FnResult<String> {
        Ok("weather".to_string())
    }

    #[plugin_fn]
    pub fn render(input: String) -> FnResult<String> {
        let v: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
        let now = v["context"]["now"].as_str().unwrap_or_default().to_string();
        let cfg = &v["config"];
        let zip = cfg["zip"].as_str().unwrap_or("48183").to_string();
        let format = cfg["format"]
            .as_str()
            .unwrap_or("{icon} {temp_f}°F")
            .to_string();
        let refresh_secs = cfg["refresh_secs"].as_i64().unwrap_or(1800);
        let api_base = cfg["api_base"]
            .as_str()
            .unwrap_or("https://wttr.in")
            .to_string();

        // 1) try fresh cache
        let cached = read_cache();
        if let Some((f_at, c_zip, temp_f, code, desc)) = &cached
            && is_fresh(&now, f_at, refresh_secs, c_zip, &zip)
        {
            return Ok(segment(&format, code, temp_f, desc, &zip));
        }

        // 2) fetch
        let url = format!("{api_base}/{zip}?format=j1");
        let fetched = unsafe { rl_http_get(url) }.ok().and_then(|r| {
            let hr: Value = serde_json::from_str(&r).ok()?;
            if hr["ok"].as_bool().unwrap_or(false) {
                parse_wttr(hr["body"].as_str().unwrap_or_default())
            } else {
                None
            }
        });

        match fetched {
            Some(w) => {
                write_cache(&now, &zip, &w);
                Ok(segment(&format, &w.code, &w.temp_f, &w.desc, &zip))
            }
            // 3) fetch failed: fall back to stale cache if any *and* it's for
            // the currently-configured zip, else empty (never show one zip's
            // weather under another zip's label).
            None => match cached {
                Some((_, c_zip, temp_f, code, desc)) if c_zip == zip => {
                    Ok(segment(&format, &code, &temp_f, &desc, &zip))
                }
                _ => Ok("[]".to_string()),
            },
        }
    }

    fn segment(format: &str, code: &str, temp_f: &str, desc: &str, zip: &str) -> String {
        let text = render_format(format, code_to_icon(code), temp_f, desc, zip);
        // one unstyled segment; the host assigns palette for left/right regions
        serde_json::json!([{ "text": text, "style": { "fg": null, "bg": null, "bold": false } }])
            .to_string()
    }

    fn read_cache() -> Option<(String, String, String, String, String)> {
        let raw = unsafe { rl_state_read("weather.json".into()) }.ok()?;
        let r: Value = serde_json::from_str(&raw).ok()?;
        if !r["ok"].as_bool().unwrap_or(false) || !r["exists"].as_bool().unwrap_or(false) {
            return None;
        }
        let c: Value = serde_json::from_str(r["contents"].as_str().unwrap_or("{}")).ok()?;
        Some((
            c["fetched_at"].as_str()?.to_string(),
            c["zip"].as_str()?.to_string(),
            c["temp_f"].as_str()?.to_string(),
            c["code"].as_str()?.to_string(),
            c["desc"].as_str().unwrap_or_default().to_string(),
        ))
    }

    fn write_cache(now: &str, zip: &str, w: &Wttr) {
        let body = serde_json::json!({
            "fetched_at": now, "zip": zip,
            "temp_f": w.temp_f, "code": w.code, "desc": w.desc,
        })
        .to_string();
        let _ = unsafe { rl_state_write("weather.json".into(), body) };
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
    fn freshness_respects_interval_and_zip() {
        let now = "2026-07-20T12:30:00-04:00";
        let recent = "2026-07-20T12:10:00-04:00"; // 20 min ago
        let old = "2026-07-20T11:00:00-04:00"; // 90 min ago
        assert!(is_fresh(now, recent, 1800, "48183", "48183")); // 20min < 30min
        assert!(!is_fresh(now, old, 1800, "48183", "48183")); // 90min > 30min
        assert!(!is_fresh(now, recent, 1800, "48183", "90210")); // zip changed
        assert!(!is_fresh(now, "garbage", 1800, "48183", "48183")); // unparseable
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
