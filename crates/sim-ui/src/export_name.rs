use sim_core::recording::Stamp;

fn component(value: &str, fallback: &str) -> String {
    let mut text = String::new();
    let mut length = 0;
    for c in value.trim().chars() {
        let c = if c.is_control() || "<>:\"/\\|?*".contains(c) {
            '-'
        } else {
            c
        };
        length += c.len_utf16();
        if length > 80 {
            break;
        }
        text.push(c);
    }
    let text = text.trim_matches([' ', '.', '-']);
    if text.is_empty() {
        fallback.into()
    } else {
        text.into()
    }
}
pub fn filename(device: &str, channel: &str, start: &Stamp, end: &Stamp) -> String {
    format!(
        "{}-{}-{}-{}-{}.mp4",
        component(device, "Dispositivo"),
        component(channel, "Canal"),
        &start.0[..10],
        start.0[11..].replace(':', "-"),
        end.0[11..].replace(':', "-")
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_clip_from_saved_names_recording_date_and_selected_times() {
        let (start, end) =
            sim_core::recording::period("2026-10-26 13:39:36", "2026-10-26 13:44:36").unwrap();
        assert_eq!(
            filename("Marinello", "CAM 2", &start, &end),
            "Marinello-CAM 2-2026-10-26-13-39-36-13-44-36.mp4"
        );
        assert_eq!(
            filename("Loja/Entrada", "CAM:2?", &start, &end),
            "Loja-Entrada-CAM-2-2026-10-26-13-39-36-13-44-36.mp4"
        );
        assert_eq!(
            filename("...", "", &start, &end),
            "Dispositivo-Canal-2026-10-26-13-39-36-13-44-36.mp4"
        );
        assert!(
            filename(&"😀".repeat(200), &"x".repeat(400), &start, &end)
                .encode_utf16()
                .count()
                < 255
        );
    }
}
