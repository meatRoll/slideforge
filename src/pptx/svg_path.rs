//! SVG path (`d` attribute) → OOXML custom-geometry path elements.
//!
//! Supports the command subset Font Awesome solid icons use (`M`/`m`,
//! `L`/`l`, `C`/`c`, `S`/`s`, `A`/`a`, `Z`/`z`, plus `H`/`V`/`Q`/`T` for
//! robustness). Arcs are converted to cubic bezier segments with the W3C
//! SVG 1.1 endpoint→center parametrization (F.6.5), each segment spanning at
//! most a quarter circle (F.6.6).
//!
//! Emits `<a:path>` children (`a:moveTo`, `a:lnTo`, `a:cubicBezTo`, ...);
//! the caller opens and closes `<a:path>` with the correct `w`/`h`.

use std::f64::consts::PI;

use super::xml::Xml;
use crate::{Error, Result};

/// A 2-D point.
type Pt = (f64, f64);

/// One geometry command in normalized absolute form.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Seg {
    MoveTo((f64, f64)),
    LineTo((f64, f64)),
    /// ctrl1, ctrl2, end.
    Cubic([(f64, f64); 3]),
    /// ctrl, end.
    Quad([(f64, f64); 2]),
    Close,
}

/// Parse `d` and write the path commands into `xml` (inside an open
/// `<a:path>` element).
pub fn emit_path_children(xml: &mut Xml, d: &str) -> Result<()> {
    for seg in parse(d)? {
        match seg {
            Seg::MoveTo(pt) => {
                xml.start("a:moveTo", &[]);
                point(xml, pt);
                xml.end("a:moveTo");
            }
            Seg::LineTo(pt) => {
                xml.start("a:lnTo", &[]);
                point(xml, pt);
                xml.end("a:lnTo");
            }
            Seg::Cubic(pts) => {
                xml.start("a:cubicBezTo", &[]);
                for pt in pts {
                    point(xml, pt);
                }
                xml.end("a:cubicBezTo");
            }
            Seg::Quad(pts) => {
                xml.start("a:quadBezTo", &[]);
                for pt in pts {
                    point(xml, pt);
                }
                xml.end("a:quadBezTo");
            }
            Seg::Close => {
                xml.leaf("a:close", &[]);
            }
        }
    }
    Ok(())
}

fn point(xml: &mut Xml, (x, y): (f64, f64)) {
    let x = x.round() as i64;
    let y = y.round() as i64;
    xml.leaf("a:pt", &[("x", &x.to_string()), ("y", &y.to_string())]);
}

/// Tokenize `d` into commands with their parameter lists. Implicit command
/// repetition is resolved up front: a run of numbers after `M` becomes
/// implicit `L` segments, after any other command it repeats that command.
pub(crate) fn parse(d: &str) -> Result<Vec<Seg>> {
    let mut p = Parser::new(d);

    let mut segs: Vec<Seg> = Vec::new();
    let mut cur = (0.0f64, 0.0f64);
    let mut start = (0.0f64, 0.0f64);
    // Previous control points for S/s and T/t reflections.
    let mut prev_cubic: Option<(f64, f64)> = None;
    let mut prev_quad: Option<(f64, f64)> = None;
    // The two most recent commands (for continuation detection).
    let mut last: Option<u8> = None;

    while let Some(cmd) = p.next_letter() {
        let relative = cmd.is_ascii_lowercase();
        let upper = cmd.to_ascii_uppercase();

        match upper {
            b'M' => {
                let mut first = true;
                while p.peek_number() {
                    let (dx, dy) = p.next_pair().ok_or_else(|| bad(d))?;
                    let abs = if relative {
                        (cur.0 + dx, cur.1 + dy)
                    } else {
                        (dx, dy)
                    };
                    if first {
                        segs.push(Seg::MoveTo(abs));
                        start = abs;
                        first = false;
                    } else {
                        segs.push(Seg::LineTo(abs));
                    }
                    cur = abs;
                    prev_cubic = None;
                    prev_quad = None;
                    last = Some(b'M');
                }
                if first {
                    return Err(bad(d));
                }
            }
            b'L' => {
                while p.peek_number() {
                    let (dx, dy) = p.next_pair().ok_or_else(|| bad(d))?;
                    let abs = if relative {
                        (cur.0 + dx, cur.1 + dy)
                    } else {
                        (dx, dy)
                    };
                    segs.push(Seg::LineTo(abs));
                    cur = abs;
                    last = Some(b'L');
                }
            }
            b'H' => {
                while p.peek_number() {
                    let x = p.next_number().ok_or_else(|| bad(d))?;
                    let abs = if relative { cur.0 + x } else { x };
                    cur = (abs, cur.1);
                    segs.push(Seg::LineTo(cur));
                    last = Some(b'H');
                }
            }
            b'V' => {
                while p.peek_number() {
                    let y = p.next_number().ok_or_else(|| bad(d))?;
                    let abs = if relative { cur.1 + y } else { y };
                    cur = (cur.0, abs);
                    segs.push(Seg::LineTo(cur));
                    last = Some(b'V');
                }
            }
            b'C' => {
                while p.peek_number() {
                    let [c1, c2, e] = p.next_triplet().ok_or_else(|| bad(d))?;
                    let c1 = if relative { add(cur, c1) } else { c1 };
                    let c2 = if relative { add(cur, c2) } else { c2 };
                    let e = if relative { add(cur, e) } else { e };
                    segs.push(Seg::Cubic([c1, c2, e]));
                    cur = e;
                    prev_cubic = Some(c2);
                    last = Some(b'C');
                }
            }
            b'S' => {
                while p.peek_number() {
                    let (c2, e) = p.next_pair2().ok_or_else(|| bad(d))?;
                    let c2 = if relative { add(cur, c2) } else { c2 };
                    let e = if relative { add(cur, e) } else { e };
                    let c1 = if matches!(last, Some(b'C') | Some(b'S')) {
                        reflect(prev_cubic, cur)
                    } else {
                        cur
                    };
                    segs.push(Seg::Cubic([c1, c2, e]));
                    cur = e;
                    prev_cubic = Some(c2);
                    last = Some(b'S');
                }
            }
            b'Q' => {
                while p.peek_number() {
                    let (c, e) = p.next_pair2().ok_or_else(|| bad(d))?;
                    let c = if relative { add(cur, c) } else { c };
                    let e = if relative { add(cur, e) } else { e };
                    segs.push(Seg::Quad([c, e]));
                    cur = e;
                    prev_quad = Some(c);
                    last = Some(b'Q');
                }
            }
            b'T' => {
                while p.peek_number() {
                    let (dx, dy) = p.next_pair().ok_or_else(|| bad(d))?;
                    let e = if relative {
                        (cur.0 + dx, cur.1 + dy)
                    } else {
                        (dx, dy)
                    };
                    let c = if matches!(last, Some(b'Q') | Some(b'T')) {
                        reflect(prev_quad, cur)
                    } else {
                        cur
                    };
                    segs.push(Seg::Quad([c, e]));
                    cur = e;
                    prev_quad = Some(c);
                    last = Some(b'T');
                }
            }
            b'A' => {
                while p.peek_number() {
                    let (rx, ry, rot, large, sweep, x, y) = p.next_arc().ok_or_else(|| bad(d))?;
                    let (x2, y2) = if relative {
                        (cur.0 + x, cur.1 + y)
                    } else {
                        (x, y)
                    };
                    if rx == 0.0 || ry == 0.0 {
                        segs.push(Seg::LineTo((x2, y2)));
                    } else {
                        for cubic in
                            arc_to_cubics(cur, rx, ry, rot, large != 0.0, sweep != 0.0, (x2, y2))
                        {
                            segs.push(Seg::Cubic(cubic));
                        }
                    }
                    cur = (x2, y2);
                    prev_cubic = None;
                    prev_quad = None;
                    last = Some(b'A');
                }
            }
            b'Z' => {
                segs.push(Seg::Close);
                cur = start;
                prev_cubic = None;
                prev_quad = None;
                last = Some(b'Z');
            }
            other => {
                return Err(Error::Invalid(format!(
                    "unsupported SVG path command `{}`",
                    other as char
                )));
            }
        }
    }

    if segs.is_empty() {
        return Err(Error::Invalid(format!("empty SVG path `{d}`")));
    }
    Ok(segs)
}

fn add(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 + b.0, a.1 + b.1)
}

fn reflect(ctrl: Option<(f64, f64)>, cur: (f64, f64)) -> (f64, f64) {
    match ctrl {
        Some((cx, cy)) => (2.0 * cur.0 - cx, 2.0 * cur.1 - cy),
        None => cur,
    }
}

fn bad(d: &str) -> Error {
    Error::Invalid(format!("malformed SVG path `{d}`"))
}

/// Endpoint → center parametrization (SVG F.6.5 / F.6.6): split the arc into
/// quarter-circle-or-smaller segments and approximate each with one cubic.
fn arc_to_cubics(
    (x1, y1): (f64, f64),
    mut rx: f64,
    mut ry: f64,
    rot_deg: f64,
    large: bool,
    sweep: bool,
    (x2, y2): (f64, f64),
) -> Vec<[(f64, f64); 3]> {
    let phi = rot_deg.to_radians();
    let (sin_p, cos_p) = phi.sin_cos();

    let dx = (x1 - x2) / 2.0;
    let dy = (y1 - y2) / 2.0;
    let x1p = cos_p * dx + sin_p * dy;
    let y1p = -sin_p * dx + cos_p * dy;

    rx = rx.abs();
    ry = ry.abs();
    let lambda = x1p * x1p / (rx * rx) + y1p * y1p / (ry * ry);
    if lambda > 1.0 {
        let scale = lambda.sqrt();
        rx *= scale;
        ry *= scale;
    }

    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let coef = if large != sweep { 1.0 } else { -1.0 } * (num / den).max(0.0).sqrt();
    let cxp = coef * (rx * y1p / ry);
    let cyp = coef * -(ry * x1p / rx);
    let cx = cos_p * cxp - sin_p * cyp + (x1 + x2) / 2.0;
    let cy = sin_p * cxp + cos_p * cyp + (y1 + y2) / 2.0;

    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;
    let theta1 = angle((ux, uy));
    let mut delta = angle_from(ux, uy, vx, vy);
    if !sweep && delta > 0.0 {
        delta -= 2.0 * PI;
    } else if sweep && delta < 0.0 {
        delta += 2.0 * PI;
    }

    let n = (delta.abs() / (PI / 2.0)).ceil().max(1.0) as usize;
    let step = delta / n as f64;
    let k = (4.0 / 3.0) * (step * 0.25).tan();

    let mut out = Vec::with_capacity(n);
    let mut t = theta1;
    for _ in 0..n {
        let t1 = t + step;
        let c1 = (
            cx + rx * (t.cos() - k * t.sin()),
            cy + ry * (t.sin() + k * t.cos()),
        );
        let c2 = (
            cx + rx * (t1.cos() + k * t1.sin()),
            cy + ry * (t1.sin() - k * t1.cos()),
        );
        let e = (cx + rx * t1.cos(), cy + ry * t1.sin());
        out.push([c1, c2, e]);
        t = t1;
    }
    out
}

fn angle((x, y): (f64, f64)) -> f64 {
    y.atan2(x)
}

fn angle_from(ux: f64, uy: f64, vx: f64, vy: f64) -> f64 {
    let dot = ux * vx + uy * vy;
    let cross = ux * vy - uy * vx;
    cross.atan2(dot)
}

/// A pull-parser over an SVG path string.
struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(d: &'a str) -> Self {
        Self {
            bytes: d.as_bytes(),
            pos: 0,
        }
    }

    fn skip(&mut self) {
        while self.pos < self.bytes.len()
            && matches!(self.bytes[self.pos], b' ' | b'\t' | b'\r' | b'\n' | b',')
        {
            self.pos += 1;
        }
    }

    fn next_letter(&mut self) -> Option<u8> {
        self.skip();
        let c = *self.bytes.get(self.pos)?;
        if c.is_ascii_alphabetic() {
            self.pos += 1;
            Some(c)
        } else {
            None
        }
    }

    fn peek_number(&mut self) -> bool {
        self.skip();
        matches!(
            self.bytes.get(self.pos),
            Some(b'0'..=b'9' | b'-' | b'+' | b'.')
        )
    }

    fn next_number(&mut self) -> Option<f64> {
        self.skip();
        let start = self.pos;
        let b = self.bytes;

        // Optional sign.
        if self.pos < b.len() && matches!(b[self.pos], b'-' | b'+') {
            self.pos += 1;
        }
        // Integer part.
        let int_digits = self.consume_digits();
        // Fraction part.
        let mut frac_digits = 0usize;
        if self.pos < b.len() && b[self.pos] == b'.' {
            self.pos += 1;
            frac_digits = self.consume_digits();
        }
        if int_digits == 0 && frac_digits == 0 {
            self.pos = start; // roll back so callers can treat it as absent
            return None;
        }
        // Exponent part (only consumed when present).
        if self.pos < b.len() && matches!(b[self.pos], b'e' | b'E') {
            let exp_start = self.pos;
            self.pos += 1;
            if self.pos < b.len() && matches!(b[self.pos], b'-' | b'+') {
                self.pos += 1;
            }
            if self.consume_digits() == 0 {
                self.pos = exp_start; // malformed exponent -> treat as not there
            }
        }
        std::str::from_utf8(&b[start..self.pos]).ok()?.parse().ok()
    }

    fn consume_digits(&mut self) -> usize {
        let start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        self.pos - start
    }

    fn next_pair(&mut self) -> Option<Pt> {
        Some((self.next_number()?, self.next_number()?))
    }

    fn next_pair2(&mut self) -> Option<(Pt, Pt)> {
        let a = self.next_pair()?;
        let b = self.next_pair()?;
        Some((a, b))
    }

    fn next_triplet(&mut self) -> Option<[Pt; 3]> {
        let a = self.next_pair()?;
        let b = self.next_pair()?;
        let c = self.next_pair()?;
        Some([a, b, c])
    }

    fn next_arc(&mut self) -> Option<(f64, f64, f64, f64, f64, f64, f64)> {
        let rx = self.next_number()?;
        let ry = self.next_number()?;
        let rot = self.next_number()?;
        let large = self.next_number()?;
        let sweep = self.next_number()?;
        let x = self.next_number()?;
        let y = self.next_number()?;
        Some((rx, ry, rot, large, sweep, x, y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_absolute_commands() {
        let segs = parse("M0 0L10 0L10 10z").unwrap();
        assert_eq!(
            segs,
            vec![
                Seg::MoveTo((0.0, 0.0)),
                Seg::LineTo((10.0, 0.0)),
                Seg::LineTo((10.0, 10.0)),
                Seg::Close,
            ]
        );
    }

    #[test]
    fn parses_relative_and_implicit_commands() {
        // m ... c ... l ... z — like the mobile-screen glyph
        let segs = parse("m16 64c0-35 29-64 64-64l224 0v0l-224 0c-35 0-64 29-64 64z").unwrap();
        assert!(matches!(segs[0], Seg::MoveTo((16.0, 64.0))));
        assert!(matches!(segs[1], Seg::Cubic(_)), "first relative c");
        assert!(matches!(segs[2], Seg::LineTo((304.0, 0.0))), "relative l");
        assert!(segs.last() == Some(&Seg::Close));
    }

    #[test]
    fn arc_full_circle_becomes_cubics() {
        // The FA gauge pattern: two half-circle arcs forming a closed ring.
        let segs = parse("M0 256a256 256 0 1 1 512 0A256 256 0 1 1 0 256z").unwrap();
        let cubics = segs.iter().filter(|s| matches!(s, Seg::Cubic(_))).count();
        assert!(
            cubics >= 4,
            "expected ≥4 cubic segments for the ring, got {cubics}"
        );
        assert!(matches!(segs[0], Seg::MoveTo((0.0, 256.0))));
        let end = segs.last().unwrap();
        assert!(matches!(end, Seg::Close), "ring must close");
    }

    #[test]
    fn invalid_command_is_rejected() {
        assert!(parse("M0 0X10 10").is_err());
    }
}
