//! The two RAM planes, their window and their address counter (datasheet §6.5, §8.2–8.5).
//!
//! An X address is a source, a pixel; a data byte holds eight pixels, the most significant bit at
//! the lowest X of the byte. The counter moves by one byte (eight pixels) in X and by one row in Y,
//! in the directions of the data-entry mode, and wraps to the start of the window past its end.

use super::config::{RAM_GATES, RAM_SOURCES};

/// Bytes in a row of a plane.
pub(crate) const ROW_BYTES: usize = RAM_SOURCES as usize / 8;
/// Bytes in a plane.
pub(crate) const PLANE_BYTES: usize = ROW_BYTES * RAM_GATES as usize;

/// The two RAM planes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Plane {
    /// Written by `0x24`: 1 is white, 0 black.
    BlackWhite,
    /// Written by `0x26`: on the X4, the previous frame for a differential refresh.
    Red,
}

/// The window of `0x44` and `0x45`, in pixels and rows, as written: with a decrementing direction
/// the start is above the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Window {
    pub(crate) x_start: u16,
    pub(crate) x_end: u16,
    pub(crate) y_start: u16,
    pub(crate) y_end: u16,
}

impl Window {
    /// The reset value: the whole RAM, 000h–3BFh by 000h–2A7h.
    pub(crate) const RESET: Window = Window {
        x_start: 0,
        x_end: RAM_SOURCES - 1,
        y_start: 0,
        y_end: RAM_GATES - 1,
    };
}

/// The address counter of `0x4E` and `0x4F`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counter {
    pub(crate) x: u16,
    pub(crate) y: u16,
}

/// The data-entry mode of `0x11`: ID0 X increments, ID1 Y increments, AM the counter moves in Y.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Entry(pub(crate) u8);

impl Entry {
    /// The reset value 03h: X and Y increment, the counter moves in X.
    pub(crate) const RESET: Entry = Entry(0x03);

    fn x_increments(self) -> bool {
        self.0 & 0b001 != 0
    }

    fn y_increments(self) -> bool {
        self.0 & 0b010 != 0
    }

    fn y_first(self) -> bool {
        self.0 & 0b100 != 0
    }
}

/// One step along an axis, in units of that axis (bytes of X, rows of Y): `Some(next)` within the
/// window, `None` past its end.
fn step(at: i32, end: i32, increments: bool) -> Option<i32> {
    if increments {
        (at < end).then_some(at + 1)
    } else {
        (at > end).then_some(at - 1)
    }
}

impl Counter {
    /// Whether the counter lies inside `window` (between start and end, whatever their order).
    pub(crate) fn within(self, window: Window) -> bool {
        let inside = |at: u16, a: u16, b: u16| at >= a.min(b) && at <= a.max(b);
        inside(self.x / 8, window.x_start / 8, window.x_end / 8)
            && inside(self.y, window.y_start, window.y_end)
    }

    /// Where the counter goes after a byte of data.
    pub(crate) fn advance(&mut self, window: Window, entry: Entry) {
        let x = i32::from(self.x / 8);
        let y = i32::from(self.y);
        let x_end = i32::from(window.x_end / 8);
        let y_end = i32::from(window.y_end);
        // Counters stay within 10 bits: the window's bounds are 10-bit values.
        let byte_to_x = |byte: i32| u16::try_from(byte * 8).unwrap_or(0);
        let to_row = |row: i32| u16::try_from(row).unwrap_or(0);
        if entry.y_first() {
            if let Some(next) = step(y, y_end, entry.y_increments()) {
                self.y = to_row(next);
                return;
            }
            self.y = window.y_start;
            self.x = match step(x, x_end, entry.x_increments()) {
                Some(next) => byte_to_x(next),
                None => window.x_start,
            };
        } else {
            if let Some(next) = step(x, x_end, entry.x_increments()) {
                self.x = byte_to_x(next);
                return;
            }
            self.x = window.x_start;
            self.y = match step(y, y_end, entry.y_increments()) {
                Some(next) => to_row(next),
                None => window.y_start,
            };
        }
    }
}

/// The two planes of 960×680 bits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ram {
    black_white: Vec<u8>,
    red: Vec<u8>,
}

impl Ram {
    /// Both planes filled with `byte`.
    pub(crate) fn filled(byte: u8) -> Ram {
        Ram {
            black_white: vec![byte; PLANE_BYTES],
            red: vec![byte; PLANE_BYTES],
        }
    }

    pub(crate) fn plane(&self, plane: Plane) -> &[u8] {
        match plane {
            Plane::BlackWhite => &self.black_white,
            Plane::Red => &self.red,
        }
    }

    fn plane_mut(&mut self, plane: Plane) -> &mut [u8] {
        match plane {
            Plane::BlackWhite => &mut self.black_white,
            Plane::Red => &mut self.red,
        }
    }

    /// The byte at the counter, `None` past the RAM.
    fn index(at: Counter) -> Option<usize> {
        let (column, row) = (usize::from(at.x / 8), usize::from(at.y));
        (column < ROW_BYTES && row < usize::from(RAM_GATES)).then_some(row * ROW_BYTES + column)
    }

    /// Writes `byte` at the counter; `false` when the counter lies past the RAM.
    pub(crate) fn write(&mut self, plane: Plane, at: Counter, byte: u8) -> bool {
        match Ram::index(at) {
            Some(index) => {
                self.plane_mut(plane)[index] = byte;
                true
            }
            None => false,
        }
    }

    /// The byte at the counter; `None` past the RAM.
    pub(crate) fn read(&self, plane: Plane, at: Counter) -> Option<u8> {
        Ram::index(at).map(|index| self.plane(plane)[index])
    }

    /// The bit of pixel (`x`, `row`), both within the RAM.
    pub(crate) fn bit(&self, plane: Plane, x: u16, row: u16) -> bool {
        let byte = self.plane(plane)[usize::from(row) * ROW_BYTES + usize::from(x / 8)];
        byte & (0x80 >> (x % 8)) != 0
    }

    /// The auto write of `0x46`/`0x47` (datasheet Table 7-1): bit 7 is the value of the first
    /// step, bits 6:4 the step height, bits 2:0 the step width; the steps alternate in a
    /// checkerboard over the whole RAM. ASSUMPTION: the whole RAM, not the window.
    pub(crate) fn auto_write(&mut self, plane: Plane, parameter: u8) {
        const HEIGHTS: [usize; 8] = [8, 16, 32, 64, 128, 256, 512, 680];
        const WIDTHS: [usize; 8] = [8, 16, 32, 64, 128, 256, 512, 960];
        let first = parameter & 0x80 != 0;
        let height = HEIGHTS[usize::from((parameter >> 4) & 0b111)];
        let width_bytes = WIDTHS[usize::from(parameter & 0b111)] / 8;
        let bytes = self.plane_mut(plane);
        for (row, line) in bytes.as_chunks_mut::<ROW_BYTES>().0.iter_mut().enumerate() {
            for (column, byte) in line.iter_mut().enumerate() {
                // The first step, and every other step after it, take the first value.
                let even_step = (row / height + column / width_bytes).is_multiple_of(2);
                *byte = if first == even_step { 0xFF } else { 0x00 };
            }
        }
    }

    /// Clears both planes to `byte`.
    pub(crate) fn fill(&mut self, byte: u8) {
        self.black_white.fill(byte);
        self.red.fill(byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(x_start: u16, x_end: u16, y_start: u16, y_end: u16) -> Window {
        Window {
            x_start,
            x_end,
            y_start,
            y_end,
        }
    }

    /// The counters a window visits, byte by byte, `count` steps from `start`.
    fn walk(start: Counter, w: Window, entry: u8, count: usize) -> Vec<(u16, u16)> {
        let mut at = start;
        (0..count)
            .map(|_| {
                let here = (at.x, at.y);
                at.advance(w, Entry(entry));
                here
            })
            .collect()
    }

    #[test]
    fn x_mode_increments_x_then_y_and_wraps_to_the_start() {
        // A window of two bytes by two rows, X then Y increment (03h).
        let w = window(8, 23, 4, 5);
        let visited = walk(Counter { x: 8, y: 4 }, w, 0x03, 5);
        assert_eq!(visited, [(8, 4), (16, 4), (8, 5), (16, 5), (8, 4)]);
    }

    #[test]
    fn the_x4_mode_increments_x_and_decrements_y() {
        // 01h: X increment, Y decrement; the window's Y start is its last row (FreeInk setRamArea).
        let w = window(0, 15, 9, 8);
        let visited = walk(Counter { x: 0, y: 9 }, w, 0x01, 5);
        assert_eq!(visited, [(0, 9), (8, 9), (0, 8), (8, 8), (0, 9)]);
    }

    #[test]
    fn x_decrement_modes_move_right_to_left() {
        // 00h: both decrement.
        let w = window(15, 0, 1, 0);
        assert_eq!(
            walk(Counter { x: 15, y: 1 }, w, 0x00, 5),
            [(15, 1), (0, 1), (15, 0), (0, 0), (15, 1)]
        );
        // 02h: X decrements, Y increments.
        let w = window(15, 0, 0, 1);
        assert_eq!(
            walk(Counter { x: 15, y: 0 }, w, 0x02, 5),
            [(15, 0), (0, 0), (15, 1), (0, 1), (15, 0)]
        );
    }

    #[test]
    fn y_mode_moves_down_the_column_first() {
        // 07h: AM = 1, X and Y increment.
        let w = window(0, 15, 0, 2);
        assert_eq!(
            walk(Counter::default(), w, 0x07, 7),
            [(0, 0), (0, 1), (0, 2), (8, 0), (8, 1), (8, 2), (0, 0)]
        );
        // 04h: AM = 1, both decrement.
        let w = window(15, 0, 1, 0);
        assert_eq!(
            walk(Counter { x: 15, y: 1 }, w, 0x04, 5),
            [(15, 1), (15, 0), (0, 1), (0, 0), (15, 1)]
        );
        // 05h and 06h: AM = 1 with one axis decrementing.
        let w = window(0, 15, 1, 0);
        assert_eq!(
            walk(Counter { x: 0, y: 1 }, w, 0x05, 3),
            [(0, 1), (0, 0), (8, 1)]
        );
        let w = window(15, 0, 0, 1);
        assert_eq!(
            walk(Counter { x: 15, y: 0 }, w, 0x06, 3),
            [(15, 0), (15, 1), (0, 0)]
        );
    }

    #[test]
    fn a_window_of_one_byte_and_one_row_stays_put() {
        let w = window(40, 47, 7, 7);
        assert_eq!(walk(Counter { x: 40, y: 7 }, w, 0x03, 3), [(40, 7); 3]);
    }

    #[test]
    fn a_counter_past_the_window_end_wraps_on_the_next_byte() {
        // The window ends at byte 1, the counter is at byte 5: it wraps.
        let w = window(0, 15, 0, 3);
        assert_eq!(walk(Counter { x: 40, y: 0 }, w, 0x03, 2), [(40, 0), (0, 1)]);
        assert!(!Counter { x: 40, y: 0 }.within(w));
        assert!(Counter { x: 15, y: 3 }.within(w));
        assert!(Counter { x: 8, y: 1 }.within(window(15, 0, 3, 0)));
    }

    #[test]
    fn writes_land_at_the_counter_and_past_the_ram_go_nowhere() {
        let mut ram = Ram::filled(0x00);
        assert!(ram.write(Plane::BlackWhite, Counter { x: 959, y: 679 }, 0xA5));
        assert_eq!(ram.plane(Plane::BlackWhite)[PLANE_BYTES - 1], 0xA5);
        assert_eq!(ram.plane(Plane::Red)[PLANE_BYTES - 1], 0x00);
        assert!(!ram.write(Plane::Red, Counter { x: 960, y: 0 }, 0xFF));
        assert!(!ram.write(Plane::Red, Counter { x: 0, y: 680 }, 0xFF));
        assert_eq!(ram.read(Plane::Red, Counter { x: 0, y: 680 }), None);
        assert_eq!(
            ram.read(Plane::BlackWhite, Counter { x: 952, y: 679 }),
            Some(0xA5)
        );
        // The most significant bit is the leftmost pixel of the byte.
        assert!(ram.bit(Plane::BlackWhite, 952, 679));
        assert!(!ram.bit(Plane::BlackWhite, 953, 679));
        assert!(ram.bit(Plane::BlackWhite, 959, 679));
        assert_ne!(Plane::Red, Plane::BlackWhite);
    }

    #[test]
    fn auto_write_fills_the_whole_ram_in_steps() {
        let mut ram = Ram::filled(0x00);
        // F7h: first step 1, steps of 680 rows and 960 sources: all white.
        ram.auto_write(Plane::BlackWhite, 0xF7);
        assert!(ram.plane(Plane::BlackWhite).iter().all(|&b| b == 0xFF));
        assert!(ram.plane(Plane::Red).iter().all(|&b| b == 0x00));
        // 00h: first step 0, 8 rows by 8 sources: a checkerboard of bytes.
        ram.auto_write(Plane::Red, 0x00);
        let red = ram.plane(Plane::Red);
        assert_eq!(
            (red[0], red[1], red[ROW_BYTES * 8], red[ROW_BYTES * 8 + 1]),
            (0, 0xFF, 0xFF, 0)
        );
        assert_eq!(red[ROW_BYTES * 7 + 2], 0x00);
        // 91h: first 1, 16 rows high, 16 sources wide.
        ram.auto_write(Plane::Red, 0x91);
        let red = ram.plane(Plane::Red);
        assert_eq!(
            (red[0], red[1], red[2], red[ROW_BYTES * 16]),
            (0xFF, 0xFF, 0, 0)
        );
        ram.fill(0x5A);
        assert!(ram.plane(Plane::Red).iter().all(|&b| b == 0x5A));
    }
}
