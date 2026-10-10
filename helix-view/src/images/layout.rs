use super::placeholder::DIACRITICS;

/// Size in cells of an image of `px` pixels: natural size, scaled down to at
/// most `max_cols`×`max_rows` keeping the aspect ratio, never enlarged.
pub fn fit(px: (u32, u32), cell_px: (u16, u16), max_cols: u16, max_rows: u16) -> (u16, u16) {
    let limit = DIACRITICS.len() as f64;
    let (cell_w, cell_h) = (cell_px.0.max(1) as f64, cell_px.1.max(1) as f64);
    let mut cols = px.0 as f64 / cell_w;
    let mut rows = px.1 as f64 / cell_h;
    for max_cols in [max_cols.max(1) as f64, limit] {
        if cols > max_cols {
            rows *= max_cols / cols;
            cols = max_cols;
        }
    }
    for max_rows in [max_rows.max(1) as f64, limit] {
        if rows > max_rows {
            cols *= max_rows / rows;
            rows = max_rows;
        }
    }
    let round = |v: f64| (v - 1e-9).ceil().max(1.0) as u16;
    (round(cols), round(rows))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn natural_size() {
        assert_eq!(fit((100, 40), (10, 20), 80, 20), (10, 2));
    }

    #[test]
    fn rounds_up_partial_cells() {
        assert_eq!(fit((101, 41), (10, 20), 80, 20), (11, 3));
    }

    #[test]
    fn fits_width() {
        // 2000x1000 px = 200x50 cells -> width 80 -> 80x20
        assert_eq!(fit((2000, 1000), (10, 20), 80, 40), (80, 20));
    }

    #[test]
    fn caps_height() {
        // 400x2000 px = 40x100 cells -> max 10 rows -> 4x10
        assert_eq!(fit((400, 2000), (10, 20), 80, 10), (4, 10));
    }

    #[test]
    fn never_upscales_and_minimum_one() {
        assert_eq!(fit((1, 1), (10, 20), 80, 20), (1, 1));
        assert_eq!(fit((5000, 1), (10, 20), 80, 20), (80, 1));
    }

    #[test]
    fn diacritic_limit() {
        let (cols, rows) = fit((100_000, 100_000), (1, 1), 1000, 1000);
        assert!(cols <= 297 && rows <= 297);
    }
}
