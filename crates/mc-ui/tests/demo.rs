//! The spike screen renders at both grid sizes, as text and as panel pixels.

use mc_ui::demo::{Demo, draw};
use mc_ui::panel::{self, COMPACT, Frame, LARGE};
use ratatui_core::backend::TestBackend;
use ratatui_core::terminal::Terminal;

fn text(cols: u16, rows: u16, d: &Demo) -> Vec<String> {
    let mut t = Terminal::new(TestBackend::new(cols, rows)).unwrap();
    t.draw(|f| draw(f, d)).unwrap();
    let buf = t.backend().buffer();
    (0..rows)
        .map(|y| (0..cols).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect()
}

#[test]
fn demo_text_large_grid() {
    let d = Demo {
        selected: 1,
        frame: 7,
        battery_percent: 87,
    };
    let lines = text(LARGE.cols, LARGE.rows, &d);
    assert_eq!(lines[0], "14:02  2 new  #7      87%");
    assert_eq!(lines[2].trim_end(), "▶ #paris");
    assert_eq!(lines[14], "A▸next  hold▸open  B◂back");
    assert_eq!(lines[9].trim_end(), "Bob: relay on the roof is");
}

#[test]
fn demo_draws_ink_on_the_panel() {
    for grid in [LARGE, COMPACT] {
        let mut frame = Frame::new();
        {
            let mut t = Terminal::new(panel::backend(&mut frame, grid)).unwrap();
            t.draw(|f| draw(f, &Demo::default())).unwrap();
        }
        let ink = frame.bits.iter().map(|b| b.count_zeros()).sum::<u32>();
        // Header bar alone is a full reversed row.
        assert!(ink > 2000, "{grid:?}: {ink} ink pixels");
        // Top-left pixel is inside the reversed header: ink.
        assert!(frame.ink(0, 0), "{grid:?}");
    }
}
