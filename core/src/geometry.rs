use crate::{PageEntry, PageInfo, Point, TextRun};

/// The rotation a page displays with once edits are applied.
pub fn display_rotation(entry: PageEntry, info: PageInfo) -> u16 {
    entry.rotation.unwrap_or(info.rotate)
}

/// Maps PDF points to CSS pixels for a page at a given scale and rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// `[a, b, c, d, e, f]` affine transform from PDF space to view space.
    transform: [f64; 6],
    pub width: f64,
    pub height: f64,
    pub scale: f64,
    pub rotation: u16,
}

impl Viewport {
    pub fn new(view: [f64; 4], rotation: u16, scale: f64) -> Self {
        let [left, bottom, right, top] = view;
        let (a, b, c, d) = match rotation % 360 {
            90 => (0.0, 1.0, 1.0, 0.0),
            180 => (-1.0, 0.0, 0.0, 1.0),
            270 => (0.0, -1.0, -1.0, 0.0),
            _ => (1.0, 0.0, 0.0, -1.0),
        };
        let (page_width, page_height) = ((right - left).abs(), (top - bottom).abs());
        let (width, height) = if rotation.is_multiple_of(180) {
            (page_width * scale, page_height * scale)
        } else {
            (page_height * scale, page_width * scale)
        };
        let (center_x, center_y) = ((left + right) / 2.0, (bottom + top) / 2.0);
        Self {
            transform: [
                a * scale,
                b * scale,
                c * scale,
                d * scale,
                width / 2.0 - a * scale * center_x - c * scale * center_y,
                height / 2.0 - b * scale * center_x - d * scale * center_y,
            ],
            width,
            height,
            scale,
            rotation: rotation % 360,
        }
    }

    /// The viewport that fits `width` CSS pixels across the rotated page.
    pub fn fit_width(view: [f64; 4], rotation: u16, width: f64) -> Self {
        let natural = Self::new(view, rotation, 1.0);
        Self::new(view, rotation, width / natural.width)
    }

    pub fn to_view(&self, point: Point) -> Point {
        let [a, b, c, d, e, f] = self.transform;
        Point {
            x: a * point.x + c * point.y + e,
            y: b * point.x + d * point.y + f,
        }
    }

    pub fn to_pdf(&self, point: Point) -> Point {
        let [a, b, c, d, e, f] = self.transform;
        let determinant = a * d - b * c;
        Point {
            x: (d * (point.x - e) - c * (point.y - f)) / determinant,
            y: (a * (point.y - f) - b * (point.x - e)) / determinant,
        }
    }
}

/// Where a text line sits on screen: its top-left corner, clockwise turn and box size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunLayout {
    pub left: f64,
    pub top: f64,
    pub rotate: u16,
    pub length: f64,
    pub height: f64,
}

impl Viewport {
    pub fn layout_run(&self, run: &TextRun) -> RunLayout {
        let rect = run.rect;
        let (right, top) = (rect.x + rect.width, rect.y + rect.height);
        let (origin, length, height) = match run.direction {
            90 => (
                Point {
                    x: rect.x,
                    y: rect.y,
                },
                rect.height,
                rect.width,
            ),
            180 => (
                Point {
                    x: right,
                    y: rect.y,
                },
                rect.width,
                rect.height,
            ),
            270 => (Point { x: right, y: top }, rect.height, rect.width),
            _ => (Point { x: rect.x, y: top }, rect.width, rect.height),
        };
        let origin = self.to_view(origin);
        RunLayout {
            left: origin.x,
            top: origin.y,
            rotate: (self.rotation + 360 - run.direction % 360) % 360,
            length: length * self.scale,
            height: height * self.scale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rect;

    const VIEW: [f64; 4] = [10.0, 20.0, 290.0, 380.0];
    const TOP_LEFT: Point = Point { x: 10.0, y: 380.0 };

    #[test]
    fn upright_pages_flip_the_y_axis_and_honor_the_crop_origin() {
        let viewport = Viewport::new(VIEW, 0, 2.0);
        assert_eq!((viewport.width, viewport.height), (560.0, 720.0));
        assert_eq!(viewport.to_view(TOP_LEFT), Point { x: 0.0, y: 0.0 });
    }

    #[test]
    fn rotations_swap_dimensions_and_send_the_top_left_clockwise() {
        let viewport = Viewport::new(VIEW, 90, 1.0);
        assert_eq!((viewport.width, viewport.height), (360.0, 280.0));
        assert_eq!(viewport.to_view(TOP_LEFT), Point { x: 360.0, y: 0.0 });
        let viewport = Viewport::new(VIEW, 180, 1.0);
        assert_eq!(viewport.to_view(TOP_LEFT), Point { x: 280.0, y: 360.0 });
        let viewport = Viewport::new(VIEW, 270, 1.0);
        assert_eq!(viewport.to_view(TOP_LEFT), Point { x: 0.0, y: 280.0 });
    }

    #[test]
    fn view_and_pdf_conversions_round_trip_at_every_rotation() {
        for rotation in [0, 90, 180, 270] {
            let viewport = Viewport::new(VIEW, rotation, 1.37);
            let point = Point { x: 123.0, y: 201.0 };
            let back = viewport.to_pdf(viewport.to_view(point));
            assert!((back.x - point.x).abs() < 1e-9 && (back.y - point.y).abs() < 1e-9);
        }
    }

    fn run(direction: u16) -> TextRun {
        TextRun {
            text: "line".into(),
            rect: Rect {
                x: 100.0,
                y: 200.0,
                width: 50.0,
                height: 12.0,
            },
            direction,
        }
    }

    #[test]
    fn upright_runs_start_at_their_top_left_corner() {
        let layout = Viewport::new(VIEW, 0, 2.0).layout_run(&run(0));
        assert_eq!(
            layout,
            RunLayout {
                left: 180.0,
                top: 336.0,
                rotate: 0,
                length: 100.0,
                height: 24.0
            }
        );
    }

    #[test]
    fn page_rotation_turns_the_run_with_it() {
        let viewport = Viewport::new(VIEW, 90, 1.0);
        let layout = viewport.layout_run(&run(0));
        assert_eq!(
            (layout.rotate, layout.length, layout.height),
            (90, 50.0, 12.0)
        );
        assert_eq!(
            viewport.to_pdf(Point {
                x: layout.left,
                y: layout.top
            }),
            Point { x: 100.0, y: 212.0 }
        );
    }

    #[test]
    fn sideways_text_on_a_rotated_page_reads_upright() {
        let viewport = Viewport::new(VIEW, 90, 1.0);
        let mut upward = run(90);
        upward.rect = Rect {
            x: 100.0,
            y: 200.0,
            width: 12.0,
            height: 50.0,
        };
        let layout = viewport.layout_run(&upward);
        assert_eq!(
            (layout.rotate, layout.length, layout.height),
            (0, 50.0, 12.0)
        );
    }

    #[test]
    fn fit_width_scales_to_the_rotated_width() {
        let viewport = Viewport::fit_width(VIEW, 90, 720.0);
        assert!((viewport.width - 720.0).abs() < 1e-9);
        assert!((viewport.scale - 2.0).abs() < 1e-9);
    }
}
