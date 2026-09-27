//! Public Shanghai environment; hourly cache, failures never block decisions.
use serde_json::{json, Value};
use std::sync::OnceLock;
use tokio::sync::Mutex;
const WEATHER: &str = "https://api.open-meteo.com/v1/forecast?latitude=31.23&longitude=121.47&current=temperature_2m,cloud_cover,surface_pressure,wind_speed_10m&timezone=Asia%2FShanghai";
const TIDE: &str =
    "https://www.sh.msa.gov.cn/shhsfb/information-aim-navigation/tide-search?PlaceId=1";
static CACHE: OnceLock<Mutex<(i64, Value)>> = OnceLock::new();
fn tide(html: &str) -> Option<Value> {
    let date = html
        .split("id=\"TideDate\"")
        .nth(1)?
        .split("value=\"")
        .nth(1)?
        .split('"')
        .next()?;
    if date.len() != 10 || !date.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return None;
    }
    let cells: Vec<&str> = html
        .split("<td>")
        .skip(1)
        .filter_map(|x| x.split("</td>").next())
        .take(10)
        .collect();
    if cells.len() != 10 || cells[0] != "潮时(Hrs)" || cells[5] != "潮高(cm)" {
        return None;
    }
    let heights: Vec<f64> = cells[6..10]
        .iter()
        .map(|x| x.parse::<f64>())
        .collect::<Result<_, _>>()
        .ok()?;
    Some(
        json!({"station":"上海吴淞","forecast_date":date,"times":cells[1..5],"heights_cm":heights,"source":TIDE,"kind":"潮汐预报，不是实时测量；仅代表 forecast_date"}),
    )
}
pub async fn snapshot() -> Value {
    // Integration tests never contact third-party services.
    if cfg!(test) {
        return json!({"source":"test","moon_phase_estimate":0.5});
    }
    let now = crate::data::now();
    let mut cache = CACHE
        .get_or_init(|| Mutex::new((0, Value::Null)))
        .lock()
        .await;
    if now - cache.0 < 3_600_000 && !cache.1.is_null() {
        return cache.1.clone();
    }
    let phase = ((now as f64 / 86_400_000.0 - 10962.75972) / 29.530588853).rem_euclid(1.0);
    let mut v = json!({"location":"上海","fetched_at":now,"moon":{"phase_fraction_estimate":phase,"method":"平均朔望月 29.530588853 天，自 2000-01-06 18:14 UTC 新月起算；近似周期，非精确星历或实时测量"},"note":"环境只提供变化与抽样混合材料，不决定情绪或要求联系；随机性主要来自系统随机源。"});
    if let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
    {
        let (weather, tides) = tokio::join!(client.get(WEATHER).send(), client.get(TIDE).send());
        v["weather"] = match weather {
            Ok(r) if r.status().is_success() => match r.json::<Value>().await {
                Ok(w) if w["current"].is_object() => {
                    json!({"source":"Open-Meteo","kind":"天气模型估计，非本地传感器","current":w["current"],"units":w["current_units"]})
                }
                _ => json!({"status":"unavailable"}),
            },
            _ => json!({"status":"unavailable"}),
        };
        v["tide"] = match tides {
            Ok(r) if r.status().is_success() => match r.text().await {
                Ok(s) => tide(&s).unwrap_or(json!({"status":"format_unavailable"})),
                _ => json!({"status":"unavailable"}),
            },
            _ => json!({"status":"unavailable"}),
        };
    }
    *cache = (now, v.clone());
    v
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tide_requires_date_and_expected_cells() {
        assert!(tide("login or error page").is_none());
        let html="id=\"TideDate\" value=\"2026-09-24\"><td>潮时(Hrs)</td><td>06:59</td><td>11:24</td><td>18:59</td><td>23:21</td><td>潮高(cm)</td><td>119</td><td>333</td><td>129</td><td>393</td>";
        let v = tide(html).unwrap();
        assert_eq!(v["forecast_date"], "2026-09-24");
        assert_eq!(v["heights_cm"][0], 119.0);
    }
}
