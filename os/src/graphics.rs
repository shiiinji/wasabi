extern crate alloc;

use alloc::vec::Vec;
use core::cmp::Ordering;

font::gen_embedded_font!();

pub trait Bitmap {
    fn bytes_per_pixel(&self) -> i64;
    fn pixels_per_line(&self) -> i64;
    fn width(&self) -> i64;
    fn height(&self) -> i64;
    fn buf(&self) -> *const u8;
    fn buf_mut(&mut self) -> *mut u8;
    fn pixel_at(&mut self, x: i64, y: i64) -> Option<&mut u32> {
        if self.is_in_x_range(x) && self.is_in_y_range(y) {
            // # Safety
            // (x, y) is always validated by the checks above.
            unsafe { Some(&mut *(self.unchecked_pixel_at(x, y))) }
        } else {
            None
        }
    }
    /// # Safety
    ///
    /// Returned pointer is valid as long as the given coordinates are valid
    /// which means that passing is_in_*_range tests.
    unsafe fn unchecked_pixel_at(&mut self, x: i64, y: i64) -> *mut u32 {
        self.buf_mut()
            .add(((y * self.pixels_per_line() + x) * self.bytes_per_pixel()) as usize)
            as *mut u32
    }
    fn flush(&self) {
        // Do nothing
    }
    fn is_in_x_range(&self, px: i64) -> bool {
        use core::cmp::min;
        0 <= px && px < min(self.width(), self.pixels_per_line())
    }
    fn is_in_y_range(&self, py: i64) -> bool {
        0 <= py && py < self.height()
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum GraphicsError {
    OutOfRange,
}

pub type GraphicsResult<T> = core::result::Result<T, GraphicsError>;

unsafe fn unchecked_draw_point<T: Bitmap>(
    buf: &mut T,
    color: u32,
    x: i64,
    y: i64,
) -> GraphicsResult<()> {
    // Assumes the buffer uses ARGB format
    // which means that the components will be stored in [A, R, G, B] order.
    // This is true for little-endian machine but not on big endian.
    *buf.unchecked_pixel_at(x, y) = color;

    Ok(())
}

#[allow(clippy::many_single_char_names)]
pub fn draw_point<T: Bitmap>(buf: &mut T, color: u32, x: i64, y: i64) -> GraphicsResult<()> {
    if !buf.is_in_x_range(x) || !buf.is_in_x_range(x) {
        return Err(GraphicsError::OutOfRange);
    }
    unsafe {
        unchecked_draw_point(buf, color, x, y)?;
    }
    Ok(())
}

pub fn draw_line<T: Bitmap>(
    buf: &mut T,
    color: u32,
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
) -> GraphicsResult<()> {
    if !buf.is_in_x_range(x0)
        || !buf.is_in_x_range(x1)
        || !buf.is_in_y_range(y0)
        || !buf.is_in_y_range(y1)
    {
        return Err(GraphicsError::OutOfRange);
    }

    if x1 < x0 {
        return draw_line(buf, color, x1, y1, x0, y0);
    }
    if x1 == x0 {
        if y0 <= y1 {
            for i in y0..=y1 {
                draw_point(buf, color, x0, i)?;
            }
        } else {
            for i in y1..=y0 {
                draw_point(buf, color, x0, i)?;
            }
        }
        return Ok(());
    }
    assert!(x0 < x1);
    let lx = x1 - x0 + 1;
    const MULTIPLIER: i64 = 1024 * 1024;
    let a = (y1 - y0) * MULTIPLIER / lx;
    for i in 0..lx {
        draw_line(
            buf,
            color,
            x0 + i,
            y0 + (a * i / MULTIPLIER),
            x0 + i,
            y0 + (a * (i + 1) / MULTIPLIER),
        )?;
    }
    draw_point(buf, color, x0, y0)?;
    draw_point(buf, color, x1, y1)?;
    Ok(())
}

pub fn draw_rect<T: Bitmap>(
    buf: &mut T,
    color: u32,
    px: i64,
    py: i64,
    w: i64,
    h: i64,
) -> GraphicsResult<()> {
    if !buf.is_in_x_range(px)
        || !buf.is_in_y_range(py)
        || !buf.is_in_x_range(px + w - 1)
        || !buf.is_in_y_range(py + h - 1)
    {
        return Err(GraphicsError::OutOfRange);
    }

    for y in py..py + h {
        for x in px..px + w {
            draw_point(buf, color, x, y)?;
        }
    }

    Ok(())
}

pub fn draw_char<T: Bitmap>(
    buf: &mut T,
    fg_color: u32,
    bg_color: u32,
    px: i64,
    py: i64,
    c: char,
) -> GraphicsResult<()> {
    if !buf.is_in_x_range(px)
        || !buf.is_in_y_range(py)
        || !buf.is_in_x_range(px + 8 - 1)
        || !buf.is_in_y_range(py + 16 - 1)
    {
        return Err(GraphicsError::OutOfRange);
    }

    let idx = c as usize;
    for y in 0..16_i64 {
        for x in 0..8_i64 {
            let col = if idx >= 256 || ((BITMAP_FONT[idx][y as usize] >> x) & 1) == 1 {
                fg_color
            } else {
                bg_color
            };
            draw_point(buf, col, px + x, py + y)?;
        }
    }

    Ok(())
}

pub struct Rect {
    x: i64,
    y: i64,
    w: i64,
    h: i64,
}
impl Rect {
    pub fn new(x: i64, y: i64, w: i64, h: i64) -> GraphicsResult<Rect> {
        if w < 0 {
            return GraphicsResult::Err(GraphicsError::OutOfRange);
        }
        if h < 0 {
            return GraphicsResult::Err(GraphicsError::OutOfRange);
        }
        GraphicsResult::Ok(Self { x, y, w, h })
    }
    pub fn x(&self) -> i64 {
        self.x
    }
    pub fn y(&self) -> i64 {
        self.y
    }
    pub fn w(&self) -> i64 {
        self.w
    }
    pub fn h(&self) -> i64 {
        self.h
    }
}

#[cfg(test)]
mod rect_tests {
    use super::Rect;

    #[test_case]
    fn creates_rect() {
        let r = Rect::new(0, 0, 0, 0).unwrap();
        assert_eq!(r.x(), 0);
        assert_eq!(r.y(), 0);
        assert_eq!(r.w(), 0);
        assert_eq!(r.h(), 0);

        let r = Rect::new(1, 2, 3, 4).unwrap();
        assert_eq!(r.x(), 1);
        assert_eq!(r.y(), 2);
        assert_eq!(r.w(), 3);
        assert_eq!(r.h(), 4);

        let r = Rect::new(-1, -2, 3, 4).unwrap();
        assert_eq!(r.x(), -1);
        assert_eq!(r.y(), -2);
        assert_eq!(r.w(), 3);
        assert_eq!(r.h(), 4);
    }
    #[test_case]
    fn fails_to_create_negative_sized_rect() {
        assert!(Rect::new(0, 0, -1, 0).is_err());
        assert!(Rect::new(0, 0, 0, -1).is_err());
        assert!(Rect::new(0, 0, -1, -1).is_err());
    }
}

/// Transfers the pixels in a rect sized (w, h) at (sx, sy) in the src bitmap
/// to (dx, dy) in the dst bitmap.
#[allow(clippy::many_single_char_names)]
pub fn draw_bmp_clipped<DstBitmap: Bitmap, SrcBitmap: Bitmap>(
    dst: &mut DstBitmap,
    src: &SrcBitmap,
    dx: i64,
    dy: i64,
) -> GraphicsResult<()> {
    let _dst_rect = Rect::new(0, 0, dst.width(), dst.height())?;
    let _src_rect = Rect::new(dx, dy, src.width(), src.height())?;

    unimplemented!("copy rect here...");
}

/// Transfers the pixels in a rect sized (w, h) from at (sx, sy) to (dx, dy).
/// Both rects should be in the buffer coordinates.
#[allow(clippy::many_single_char_names)]
pub fn transfer_rect<T: Bitmap>(
    buf: &mut T,
    dx: i64,
    dy: i64,
    sx: i64,
    sy: i64,
    w: i64,
    h: i64,
) -> GraphicsResult<()> {
    if !buf.is_in_x_range(sx)
        || !buf.is_in_y_range(sy)
        || !buf.is_in_x_range(sx + w - 1)
        || !buf.is_in_y_range(sy + h - 1)
        || !buf.is_in_x_range(dx)
        || !buf.is_in_y_range(dy)
        || !buf.is_in_x_range(dx + w - 1)
        || !buf.is_in_y_range(dy + h - 1)
    {
        return Err(GraphicsError::OutOfRange);
    }

    match (dy.cmp(&sy), dx.cmp(&sx)) {
        (Ordering::Less, _) | (Ordering::Equal, Ordering::Less) => {
            for y in 0..h {
                for x in 0..w {
                    unsafe {
                        let c = *buf.unchecked_pixel_at(sx + x, sy + y);
                        unchecked_draw_point(buf, c, dx + x, dy + y)?;
                    }
                }
            }
        }
        (Ordering::Greater, _) => {
            for y in (0..h).rev() {
                for x in 0..w {
                    unsafe {
                        let c = *buf.unchecked_pixel_at(sx + x, sy + y);
                        unchecked_draw_point(buf, c, dx + x, dy + y)?;
                    }
                }
            }
        }
        (Ordering::Equal, Ordering::Greater) => {
            for y in 0..h {
                for x in (0..w).rev() {
                    unsafe {
                        let c = *buf.unchecked_pixel_at(sx + x, sy + y);
                        unchecked_draw_point(buf, c, dx + x, dy + y)?;
                    }
                }
            }
        }
        (Ordering::Equal, Ordering::Equal) => {
            // Do nothing
        }
    }
    Ok(())
}

pub struct BitmapBuffer {
    buf: Vec<u8>,
    width: i64,
    height: i64,
    pixels_per_line: i64,
}
impl BitmapBuffer {
    pub fn new(width: i64, height: i64, pixels_per_line: i64) -> Self {
        assert!(width >= 0);
        assert!(height >= 0);
        assert!(pixels_per_line >= 0);
        assert!(pixels_per_line >= width);
        let mut buf = Self {
            buf: Vec::new(),
            width,
            height,
            pixels_per_line,
        };
        buf.buf.resize((pixels_per_line * height * 4) as usize, 0);
        buf
    }
}
impl Bitmap for BitmapBuffer {
    fn bytes_per_pixel(&self) -> i64 {
        4
    }
    fn pixels_per_line(&self) -> i64 {
        self.pixels_per_line
    }
    fn width(&self) -> i64 {
        self.width
    }
    fn height(&self) -> i64 {
        self.height
    }
    fn buf(&self) -> *const u8 {
        self.buf.as_ptr()
    }
    fn buf_mut(&mut self) -> *mut u8 {
        self.buf.as_mut_ptr()
    }
}

#[cfg(test)]
mod tests {
    use super::Bitmap;
    use super::*;
    use alloc::vec;

    #[test_case]
    fn test_buf_default() {
        let h = 13_i64;
        let w = 17_i64;
        let pixels_per_line = 19_i64;
        let mut buf = BitmapBuffer::new(w, h, pixels_per_line);
        assert!(buf.pixel_at(-1, 0).is_none());
        assert!(buf.pixel_at(0, -1).is_none());
        assert!(buf.pixel_at(w - 1, h).is_none());
        assert!(buf.pixel_at(w, h - 1).is_none());
        for y in 0..h {
            for x in 0..w {
                assert!(buf.pixel_at(x, y) == Some(&mut 0))
            }
        }
    }
    #[test_case]
    fn draw_rect_default() {
        let h = 13_i64;
        let w = 17_i64;
        let pixels_per_line = 19_i64;
        let mut buf = BitmapBuffer::new(w, h, pixels_per_line);
        assert!(draw_rect(&mut buf, 0xff0000, 0, 0, w, h).is_ok());
        for y in 0..h {
            for x in 0..w {
                assert!(buf.pixel_at(x, y) == Some(&mut 0xff0000))
            }
        }
    }
    mod transfer_rect {
        use super::*;

        const H: i64 = 4;
        const W: i64 = 4;
        const PIXELS_PER_LINE: i64 = 4;
        #[rustfmt::skip]
        const INIT: [[u32; W as usize]; H as usize] = [
            [0u32,   1u32,  2u32,  3u32],
            [10u32, 11u32, 12u32, 13u32],
            [20u32, 21u32, 22u32, 23u32],
            [30u32, 31u32, 32u32, 33u32],
        ];
        #[rustfmt::skip]
        const TR_U: [[u32; W as usize]; H as usize] = [
            [0u32,  11u32, 12u32,  3u32],
            [10u32, 21u32, 22u32, 13u32],
            [20u32, 21u32, 22u32, 23u32],
            [30u32, 31u32, 32u32, 33u32],
        ];
        #[rustfmt::skip]
        const TR_D: [[u32; W as usize]; H as usize] = [
            [0u32,   1u32,  2u32,  3u32],
            [10u32, 11u32, 12u32, 13u32],
            [20u32, 11u32, 12u32, 23u32],
            [30u32, 21u32, 22u32, 33u32],
        ];
        #[rustfmt::skip]
        const TR_L: [[u32; W as usize]; H as usize] = [
            [0u32,  1u32,   2u32,  3u32],
            [11u32, 12u32, 12u32, 13u32],
            [21u32, 22u32, 22u32, 23u32],
            [30u32, 31u32, 32u32, 33u32],
        ];
        #[rustfmt::skip]
        const TR_R: [[u32; W as usize]; H as usize] = [
            [0u32,  1u32,   2u32,  3u32],
            [10u32, 11u32, 11u32, 12u32],
            [20u32, 21u32, 21u32, 22u32],
            [30u32, 31u32, 32u32, 33u32],
        ];

        #[test_case]
        fn transfer_rect_all_dir() {
            let mut buf = BitmapBuffer::new(W, H, PIXELS_PER_LINE);
            let dirs = vec![
                (0, 0, &INIT),
                (-1, 0, &TR_L),
                (1, 0, &TR_R),
                (0, -1, &TR_U),
                (0, 1, &TR_D),
            ];
            for (dx, dy, expected) in dirs {
                // Init state
                for y in 0..H {
                    for x in 0..W {
                        unsafe {
                            *buf.unchecked_pixel_at(x, y) = INIT[y as usize][x as usize];
                        }
                    }
                }
                // Transfer
                transfer_rect(&mut buf, 1 + dx, 1 + dy, 1, 1, 2, 2).unwrap();
                // Check
                for y in 0..H {
                    for x in 0..W {
                        unsafe {
                            assert_eq!(
                                *buf.unchecked_pixel_at(x, y),
                                expected[y as usize][x as usize]
                            );
                        }
                    }
                }
            }
        }
    }
}
