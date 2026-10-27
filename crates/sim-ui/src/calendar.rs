use crate::CalendarDay;
use sim_core::recording::Stamp;

pub fn shift(year: i32, month: i32, delta: i32) -> Option<(i32, i32)> {
    let index = year * 12 + month - 1 + delta;
    let (year, month) = (index.div_euclid(12), index.rem_euclid(12) + 1);
    (2000..=2099).contains(&year).then_some((year, month))
}
pub fn cells(
    year: i32,
    month: i32,
    selected: (i32, i32, i32),
    today: (i32, i32, i32),
) -> Vec<CalendarDay> {
    let offsets = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if month < 3 { year - 1 } else { year };
    let first = (y + y / 4 - y / 100 + y / 400 + offsets[(month - 1) as usize] + 1) % 7;
    let count = (28..=31)
        .rev()
        .find(|day| Stamp::parse(&format!("{year:04}-{month:02}-{day:02} 00:00:00")).is_ok())
        .unwrap();
    (0..42)
        .map(|index| {
            let day = index - first + 1;
            let number = if (1..=count).contains(&day) { day } else { 0 };
            CalendarDay {
                number,
                selected: number > 0 && selected == (year, month, number),
                today: number > 0 && today == (year, month, number),
            }
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calendar_uses_sunday_first_and_handles_leap_years() {
        let october = cells(2026, 10, (2026, 10, 26), (2026, 10, 26));
        assert_eq!(october.len(), 42);
        assert!(october[..4].iter().all(|d| d.number == 0));
        assert_eq!(october[4].number, 1);
        assert_eq!(october[34].number, 31);
        assert_eq!(october.iter().filter(|d| d.selected && d.today).count(), 1);
        assert_eq!(
            cells(2024, 2, (0, 0, 0), (0, 0, 0))
                .iter()
                .filter(|d| d.number > 0)
                .count(),
            29
        );
        assert_eq!(
            cells(2026, 2, (0, 0, 0), (0, 0, 0))
                .iter()
                .filter(|d| d.number > 0)
                .count(),
            28
        );
        assert_eq!(cells(2026, 11, (0, 0, 0), (0, 0, 0))[0].number, 1);
    }
    #[test]
    fn navigation_crosses_years_without_leaving_supported_range() {
        assert_eq!(shift(2026, 12, 1), Some((2027, 1)));
        assert_eq!(shift(2026, 1, -1), Some((2025, 12)));
        assert_eq!(shift(2000, 1, -1), None);
        assert_eq!(shift(2099, 12, 1), None);
    }
}
