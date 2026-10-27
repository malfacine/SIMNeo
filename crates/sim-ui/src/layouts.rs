use crate::{AppWindow, LayoutChoice, TileRect};
use slint::VecModel;

pub const COUNTS: [i32; 18] = [1, 4, 9, 16, 25, 36, 49, 64, 6, 8, 10, 12, 14, 4, 5, 6, 7, 8];

pub fn initialize(ui: &AppWindow) {
    let choices: Vec<_> = COUNTS
        .iter()
        .enumerate()
        .map(|(preset, &count)| {
            let kind = match preset {
                0..=7 => "grade",
                8..=12 => "destaque",
                _ => "faixa inferior",
            };
            LayoutChoice {
                count,
                label: format!("{count} câmeras · {kind}").into(),
                cells: std::rc::Rc::new(VecModel::from(rectangles(preset))).into(),
            }
        })
        .collect();
    ui.set_layout_choices(std::rc::Rc::new(VecModel::from(choices)).into());
}

pub fn index(preset: Option<usize>, count: i32) -> usize {
    preset
        .filter(|&i| COUNTS.get(i) == Some(&count))
        .or_else(|| COUNTS.iter().position(|&n| n == count))
        .unwrap_or(1)
}

pub fn rectangles(preset: usize) -> Vec<TileRect> {
    let mut cells = Vec::new();
    let mut push = |x: f32, y: f32, width: f32, height: f32| {
        cells.push(TileRect {
            x,
            y,
            width,
            height,
        });
    };
    match preset {
        0..=7 => {
            let side = (preset + 1) as f32;
            for y in 0..=preset {
                for x in 0..=preset {
                    push(x as f32 / side, y as f32 / side, 1.0 / side, 1.0 / side);
                }
            }
        }
        8..=12 => {
            let side = (preset - 5) as f32;
            let small = 1.0 / side;
            let big = 1.0 - small;
            push(0.0, 0.0, big, big);
            for row in 0..side as usize - 1 {
                push(big, row as f32 * small, small, small);
            }
            for col in 0..side as usize {
                push(col as f32 * small, big, small, small);
            }
        }
        13..=17 => {
            let columns = (preset - 10) as f32;
            let small = 1.0 / columns;
            push(0.0, 0.0, 1.0, 1.0 - small);
            for col in 0..columns as usize {
                push(col as f32 * small, 1.0 - small, small, small);
            }
        }
        _ => return rectangles(1),
    }
    cells
}

pub fn apply(ui: &AppWindow, preset: usize) {
    let preset = preset.min(COUNTS.len() - 1);
    let mut cells = rectangles(preset);
    cells.resize(crate::mosaic::CAPACITY, TileRect::default());
    ui.set_tile_rects(std::rc::Rc::new(VecModel::from(cells)).into());
    ui.set_layout_index(preset as i32);
    ui.set_layout_size(COUNTS[preset]);
    if ui.get_selected_slot() >= COUNTS[preset] {
        ui.set_selected_slot(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_cover_viewport_without_overlaps_or_gaps() {
        for (preset, count) in COUNTS.iter().enumerate() {
            let cells = rectangles(preset);
            assert_eq!(cells.len(), *count as usize);
            let area: f32 = cells.iter().map(|c| c.width * c.height).sum();
            assert!((area - 1.0).abs() < 0.0001);
            for (i, a) in cells.iter().enumerate() {
                assert!(a.x >= 0.0 && a.y >= 0.0 && a.width > 0.0 && a.height > 0.0);
                assert!(a.x + a.width <= 1.0001 && a.y + a.height <= 1.0001);
                for b in &cells[i + 1..] {
                    let width = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x);
                    let height = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
                    assert!(width < 0.0001 || height < 0.0001, "preset {preset}");
                }
            }
        }
    }
}
