//! Datas de gravação no horário local do DVR, sem conversão implícita de fuso.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Stamp(pub String);
impl Stamp {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        let b = value.as_bytes();
        if b.len() != 19
            || b[4] != b'-'
            || b[7] != b'-'
            || b[10] != b' '
            || b[13] != b':'
            || b[16] != b':'
        {
            return Err("Use data AAAA-MM-DD e hora HH:MM:SS.");
        }
        if b.iter()
            .enumerate()
            .any(|(i, c)| ![4, 7, 10, 13, 16].contains(&i) && !c.is_ascii_digit())
        {
            return Err("Data ou hora inválida.");
        }
        let n = |a, b| value[a..b].parse::<u32>().unwrap();
        let (y, m, d, h, min, s) = (n(0, 4), n(5, 7), n(8, 10), n(11, 13), n(14, 16), n(17, 19));
        let days = match m {
            4 | 6 | 9 | 11 => 30,
            2 => {
                if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
                    29
                } else {
                    28
                }
            }
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            _ => 0,
        };
        if !(2000..=2099).contains(&y) || d == 0 || d > days || h > 23 || min > 59 || s > 59 {
            return Err("Data ou hora inválida.");
        }
        Ok(Self(value.into()))
    }
    pub fn second(&self) -> u32 {
        self.0[11..13].parse::<u32>().unwrap() * 3600
            + self.0[14..16].parse::<u32>().unwrap() * 60
            + self.0[17..19].parse::<u32>().unwrap()
    }
    pub fn at_second(&self, second: u32) -> Self {
        let s = second.min(86399);
        Self(format!(
            "{} {:02}:{:02}:{:02}",
            &self.0[..10],
            s / 3600,
            s / 60 % 60,
            s % 60
        ))
    }
    pub fn rtsp(&self) -> String {
        self.0.replace(['-', ' ', ':'], "_")
    }
}
pub fn period(start: &str, end: &str) -> Result<(Stamp, Stamp), &'static str> {
    let start = Stamp::parse(start)?;
    let end = Stamp::parse(end)?;
    if start >= end {
        return Err("O horário final deve ser posterior ao inicial.");
    }
    if start.0[..10] != end.0[..10] {
        return Err("Pesquise um dia de cada vez.");
    }
    Ok((start, end))
}
#[derive(Clone, Debug)]
pub struct Recording {
    pub channel: u16,
    pub start: Stamp,
    pub end: Stamp,
    pub kind: String,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_calendar_period_and_rtsp_time() {
        assert!(Stamp::parse("2026-02-29 12:00:00").is_err());
        assert!(Stamp::parse("2024-02-29 23:59:59").is_ok());
        assert!(Stamp::parse("2026-10-26 24:00:00").is_err());
        assert!(period("2026-10-26 12:00:00", "2026-10-26 11:00:00").is_err());
        assert!(period("2026-10-26 12:00:00", "2026-10-27 11:00:00").is_err());
        let (a, _) = period("2026-10-26 00:00:00", "2026-10-26 23:59:59").unwrap();
        assert_eq!(a.at_second(3661).rtsp(), "2026_10_26_01_01_01");
    }
}
