//! Google Analytics 4 through the server-side Measurement Protocol: orders,
//! payments and sign-ups become GA4 events. No browser tag, so the panel's
//! CSP stays closed and no visitor cookies are set. Ids sent to Google are
//! SHA-256 pseudonyms, never emails or names.
use pelty_addon_sdk::serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub fn pseudonym(id: &str) -> String {
    let h = Sha256::digest(format!("pelty-ga:{id}").as_bytes());
    let n = u64::from_be_bytes(h[..8].try_into().expect("8 bytes"));
    format!("{}.{}", n >> 32, n & 0xffff_ffff)
}

pub fn valid_measurement_id(m: &str) -> bool {
    m.len() > 2
        && m.len() <= 32
        && m.starts_with("G-")
        && m[2..].chars().all(|c| c.is_ascii_alphanumeric())
}

/// GA4 event for a PeltyCMS domain event, or None if it is not tracked.
pub fn ga_event(name: &str, p: &Value) -> Option<Value> {
    let s = |k: &str| {
        p.get(k).and_then(|v| {
            v.as_str()
                .map(str::to_string)
                .or_else(|| (!v.is_null()).then(|| v.to_string()))
        })
    };
    let (client, ev) = match name {
        "order.created" => {
            let total = s("total")?;
            // Money stays a decimal string; parse only for GA's numeric field.
            let value: f64 = total.trim().parse().ok()?;
            (
                s("customer_id")?,
                json!({ "name": "purchase", "params": { "transaction_id": s("order_id")?, "value": value, "currency": s("currency")?.to_ascii_uppercase() } }),
            )
        }
        "invoice.paid" => (
            s("invoice_id")?,
            json!({ "name": "invoice_paid", "params": { "invoice_id": pseudonym(&s("invoice_id")?) } }),
        ),
        "user.registered" => (
            s("user_id")?,
            json!({ "name": "sign_up", "params": { "method": s("via").unwrap_or_else(|| "password".into()) } }),
        ),
        _ => return None,
    };
    Some(json!({ "client_id": pseudonym(&client), "non_personalized_ads": true, "events": [ev] }))
}

pub fn collect_url(measurement_id: &str, api_secret: &str) -> Option<String> {
    if !valid_measurement_id(measurement_id)
        || api_secret.is_empty()
        || !api_secret
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some(format!("https://www.google-analytics.com/mp/collect?measurement_id={measurement_id}&api_secret={api_secret}"))
}

#[cfg(target_family = "wasm")]
mod export {
    use super::*;
    use pelty_addon_sdk::serde_json;
    struct PeltyAddon;
    impl pelty_addon_sdk::Guest for PeltyAddon {
        fn describe() -> String {
            json!({ "id": "google_analytics", "name": "Google Analytics 4" }).to_string()
        }
        fn call(op: String, _input: String) -> Result<String, String> {
            Err(format!("unknown op {op}"))
        }
        fn on_event(name: String, payload: String) -> Result<(), String> {
            let p: Value = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
            let Some(body) = ga_event(&name, &p) else {
                return Ok(());
            };
            let get = |k: &str| {
                pelty_addon_sdk::setting(k)
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default()
            };
            let url = collect_url(get("measurement_id").trim(), get("api_secret").trim())
                .ok_or("Google Analytics is not configured")?;
            let (st, _) = pelty_addon_sdk::fetch(
                "POST",
                &url,
                vec![("content-type".into(), "application/json".into())],
                body.to_string(),
            )?;
            if (200..300).contains(&st) {
                Ok(())
            } else {
                Err(format!("GA4 returned {st}"))
            }
        }
    }
    pelty_addon_sdk::export!(PeltyAddon with_types_in pelty_addon_sdk);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn order_becomes_purchase_without_pii() {
        let e = ga_event("order.created", &json!({ "order_id": "o1", "customer_id": "c1", "total": "49.5000", "currency": "eur" })).unwrap();
        assert_eq!(
            e["events"][0],
            json!({ "name": "purchase", "params": { "transaction_id": "o1", "value": 49.5, "currency": "EUR" } })
        );
        assert_eq!(e["client_id"], pseudonym("c1"));
        assert!(!e.to_string().contains("c1\""));
        assert_eq!(pseudonym("c1"), pseudonym("c1"));
        assert_ne!(pseudonym("c1"), pseudonym("c2"));
    }
    #[test]
    fn other_events() {
        assert_eq!(
            ga_event(
                "user.registered",
                &json!({ "user_id": "u", "via": "github" })
            )
            .unwrap()["events"][0]["params"]["method"],
            "github"
        );
        assert_eq!(
            ga_event("invoice.paid", &json!({ "invoice_id": "i" })).unwrap()["events"][0]["name"],
            "invoice_paid"
        );
        assert!(ga_event("ticket.closed", &json!({})).is_none());
        assert!(ga_event(
            "order.created",
            &json!({ "order_id": "o", "customer_id": "c", "total": "x", "currency": "EUR" })
        )
        .is_none());
    }
    #[test]
    fn url_rejects_bad_config() {
        assert_eq!(
            collect_url("G-ABC123", "s3cr-t").unwrap(),
            "https://www.google-analytics.com/mp/collect?measurement_id=G-ABC123&api_secret=s3cr-t"
        );
        assert!(collect_url("UA-1", "s").is_none());
        assert!(collect_url("G-ABC&x=1", "s").is_none());
        assert!(collect_url("G-ABC", "a&b").is_none());
        assert!(collect_url("G-ABC", "").is_none());
    }
}
