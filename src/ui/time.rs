use jiff::{tz::TimeZone, Timestamp, Zoned};

/// Discord's ISO 8601 timestamp as a short local label: `HH:MM` for
/// today's messages, `YYYY-MM-DD HH:MM` for older ones. Unparseable
/// values pass through unchanged.
pub fn format_timestamp(raw: &str) -> String {
    let Ok(ts) = raw.parse::<Timestamp>() else {
        return raw.to_owned();
    };
    let zoned = ts.to_zoned(TimeZone::system());
    let format = if zoned.date() == Zoned::now().date() {
        "%H:%M"
    } else {
        "%Y-%m-%d %H:%M"
    };
    zoned.strftime(format).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn today_shows_time_only() {
        let now = Timestamp::now().to_string();
        let label = format_timestamp(&now);
        assert_eq!(label.len(), 5);
        assert_eq!(&label[2..3], ":");
    }

    #[test]
    fn older_messages_include_date() {
        let label = format_timestamp("2020-06-15T12:00:00Z");
        assert!(label.contains('-') && label.contains(':'));
        assert_eq!(label.len(), 16);
    }

    #[test]
    fn unparseable_passes_through() {
        assert_eq!(format_timestamp("now"), "now");
    }
}
