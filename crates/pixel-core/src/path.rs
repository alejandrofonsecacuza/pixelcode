//! SVG path parser (`d="M 0 0 L 10 0 ..."`) and curve flattening into polylines.
//!
//! Supports M L H V C S Q T A Z, absolute and relative. Curves are flattened with Wang's
//! formula and arcs by sagitta error, with a sub-pixel tolerance.

#[derive(Clone, Debug, PartialEq)]
pub struct SubPath {
    pub pts: Vec<(f64, f64)>,
    pub closed: bool,
}

type P = (f64, f64);

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl Lexer<'_> {
    fn skip(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r' | b',')
        {
            self.i += 1;
        }
    }

    fn command(&mut self) -> Option<u8> {
        self.skip();
        let c = *self.s.get(self.i)?;
        if c.is_ascii_alphabetic() && c != b'e' && c != b'E' {
            self.i += 1;
            Some(c)
        } else {
            None
        }
    }

    fn at_end(&mut self) -> bool {
        self.skip();
        self.i >= self.s.len()
    }

    fn has_number(&mut self) -> bool {
        self.skip();
        matches!(self.s.get(self.i), Some(b'0'..=b'9' | b'-' | b'+' | b'.'))
    }

    fn number(&mut self) -> Result<f64, String> {
        self.skip();
        let start = self.i;
        let s = self.s;
        if matches!(s.get(self.i), Some(b'-' | b'+')) {
            self.i += 1;
        }
        let mut digits = 0;
        while matches!(s.get(self.i), Some(b'0'..=b'9')) {
            self.i += 1;
            digits += 1;
        }
        if s.get(self.i) == Some(&b'.') {
            self.i += 1;
            while matches!(s.get(self.i), Some(b'0'..=b'9')) {
                self.i += 1;
                digits += 1;
            }
        }
        if digits > 0 && matches!(s.get(self.i), Some(b'e' | b'E')) {
            let save = self.i;
            self.i += 1;
            if matches!(s.get(self.i), Some(b'-' | b'+')) {
                self.i += 1;
            }
            if matches!(s.get(self.i), Some(b'0'..=b'9')) {
                while matches!(s.get(self.i), Some(b'0'..=b'9')) {
                    self.i += 1;
                }
            } else {
                self.i = save;
            }
        }
        if digits == 0 {
            return Err(format!("path: expected a number at byte {start}"));
        }
        std::str::from_utf8(&s[start..self.i])
            .unwrap()
            .parse::<f64>()
            .map_err(|e| format!("path: {e} at byte {start}"))
    }

    fn flag(&mut self) -> Result<bool, String> {
        self.skip();
        match self.s.get(self.i) {
            Some(b'0') => {
                self.i += 1;
                Ok(false)
            }
            Some(b'1') => {
                self.i += 1;
                Ok(true)
            }
            _ => Err(format!("path: expected arc flag 0/1 at byte {}", self.i)),
        }
    }

    fn pair(&mut self) -> Result<P, String> {
        Ok((self.number()?, self.number()?))
    }
}

pub fn parse(d: &str, tol: f64) -> Result<Vec<SubPath>, String> {
    let tol = tol.max(0.01);
    let mut lx = Lexer {
        s: d.as_bytes(),
        i: 0,
    };
    let mut out: Vec<SubPath> = Vec::new();
    let mut cur_path: Option<SubPath> = None;
    let mut cur: P = (0.0, 0.0);
    let mut start: P = (0.0, 0.0);
    let mut cmd: Option<u8> = None;
    let mut last_c: Option<P> = None;
    let mut last_q: Option<P> = None;

    fn ensure(cp: &mut Option<SubPath>, cur: P) -> &mut SubPath {
        cp.get_or_insert_with(|| SubPath {
            pts: vec![cur],
            closed: false,
        })
    }

    loop {
        if let Some(c) = lx.command() {
            cmd = Some(c);
        } else if lx.at_end() {
            break;
        } else if !lx.has_number() {
            return Err(format!("path: unexpected character at byte {}", lx.i));
        }
        let Some(c) = cmd else {
            return Err("path: must start with a command (M)".into());
        };
        if out.is_empty() && cur_path.is_none() && !matches!(c, b'M' | b'm') {
            return Err("path: must start with a moveto command (M x y)".into());
        }
        let rel = c.is_ascii_lowercase();
        let off = |p: P, cur: P| if rel { (p.0 + cur.0, p.1 + cur.1) } else { p };
        let upper = c.to_ascii_uppercase();
        let mut next_c = None;
        let mut next_q = None;
        match upper {
            b'M' => {
                let p = off(lx.pair()?, cur);
                if let Some(sp) = cur_path.take() {
                    out.push(sp);
                }
                cur_path = Some(SubPath {
                    pts: vec![p],
                    closed: false,
                });
                cur = p;
                start = p;
                cmd = Some(if rel { b'l' } else { b'L' });
            }
            b'L' => {
                let p = off(lx.pair()?, cur);
                ensure(&mut cur_path, cur).pts.push(p);
                cur = p;
            }
            b'H' => {
                let x = lx.number()?;
                let p = (if rel { cur.0 + x } else { x }, cur.1);
                ensure(&mut cur_path, cur).pts.push(p);
                cur = p;
            }
            b'V' => {
                let y = lx.number()?;
                let p = (cur.0, if rel { cur.1 + y } else { y });
                ensure(&mut cur_path, cur).pts.push(p);
                cur = p;
            }
            b'C' | b'S' => {
                let c1 = if upper == b'C' {
                    off(lx.pair()?, cur)
                } else {
                    last_c.map_or(cur, |lc| (2.0 * cur.0 - lc.0, 2.0 * cur.1 - lc.1))
                };
                let c2 = off(lx.pair()?, cur);
                let p = off(lx.pair()?, cur);
                cubic(cur, c1, c2, p, tol, &mut ensure(&mut cur_path, cur).pts);
                next_c = Some(c2);
                cur = p;
            }
            b'Q' | b'T' => {
                let q = if upper == b'Q' {
                    off(lx.pair()?, cur)
                } else {
                    last_q.map_or(cur, |lq| (2.0 * cur.0 - lq.0, 2.0 * cur.1 - lq.1))
                };
                let p = off(lx.pair()?, cur);
                quad(cur, q, p, tol, &mut ensure(&mut cur_path, cur).pts);
                next_q = Some(q);
                cur = p;
            }
            b'A' => {
                let rx = lx.number()?;
                let ry = lx.number()?;
                let rot = lx.number()?;
                let large = lx.flag()?;
                let sweep = lx.flag()?;
                let p = off(lx.pair()?, cur);
                arc(
                    cur,
                    rx,
                    ry,
                    rot,
                    large,
                    sweep,
                    p,
                    tol,
                    &mut ensure(&mut cur_path, cur).pts,
                );
                cur = p;
            }
            b'Z' => {
                if let Some(mut sp) = cur_path.take() {
                    sp.closed = true;
                    out.push(sp);
                }
                cur = start;
                cmd = None;
            }
            _ => return Err(format!("path: unknown command '{}'", c as char)),
        }
        last_c = next_c;
        last_q = next_q;
    }
    if let Some(sp) = cur_path.take() {
        out.push(sp);
    }
    Ok(out)
}

fn quad(p0: P, p1: P, p2: P, tol: f64, out: &mut Vec<P>) {
    let dd = (p0.0 - 2.0 * p1.0 + p2.0).hypot(p0.1 - 2.0 * p1.1 + p2.1);
    let n = ((0.25 * dd / tol).sqrt().ceil() as usize).clamp(1, 1000);
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let u = 1.0 - t;
        out.push((
            u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0,
            u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1,
        ));
    }
}

fn cubic(p0: P, p1: P, p2: P, p3: P, tol: f64, out: &mut Vec<P>) {
    let d1 = (p0.0 - 2.0 * p1.0 + p2.0).hypot(p0.1 - 2.0 * p1.1 + p2.1);
    let d2 = (p1.0 - 2.0 * p2.0 + p3.0).hypot(p1.1 - 2.0 * p2.1 + p3.1);
    let n = ((0.75 * d1.max(d2) / tol).sqrt().ceil() as usize).clamp(1, 1000);
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        out.push((
            a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
            a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
        ));
    }
}

/// SVG elliptical arc (endpoint-to-center conversion, W3C SVG F.6.5).
#[allow(clippy::too_many_arguments)]
fn arc(
    p0: P,
    rx: f64,
    ry: f64,
    rot_deg: f64,
    large: bool,
    sweep: bool,
    p1: P,
    tol: f64,
    out: &mut Vec<P>,
) {
    if p0 == p1 {
        return;
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-12 || ry < 1e-12 {
        out.push(p1);
        return;
    }
    let phi = rot_deg.to_radians();
    let (cs, sn) = (phi.cos(), phi.sin());
    let (dx2, dy2) = ((p0.0 - p1.0) / 2.0, (p0.1 - p1.1) / 2.0);
    let x1p = cs * dx2 + sn * dy2;
    let y1p = -sn * dx2 + cs * dy2;
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = if den > 0.0 {
        (num / den).max(0.0).sqrt()
    } else {
        0.0
    };
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1p / ry;
    let cyp = -coef * ry * x1p / rx;
    let cx = cs * cxp - sn * cyp + (p0.0 + p1.0) / 2.0;
    let cy = sn * cxp + cs * cyp + (p0.1 + p1.1) / 2.0;
    let ang = |ux: f64, uy: f64, vx: f64, vy: f64| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    let (ux, uy) = ((x1p - cxp) / rx, (y1p - cyp) / ry);
    let (vx, vy) = ((-x1p - cxp) / rx, (-y1p - cyp) / ry);
    let theta1 = ang(1.0, 0.0, ux, uy);
    let mut dtheta = ang(ux, uy, vx, vy);
    if !sweep && dtheta > 0.0 {
        dtheta -= std::f64::consts::TAU;
    } else if sweep && dtheta < 0.0 {
        dtheta += std::f64::consts::TAU;
    }
    let r = rx.max(ry);
    let step = if tol < r {
        2.0 * (1.0 - tol / r).acos()
    } else {
        std::f64::consts::FRAC_PI_2
    };
    let n = ((dtheta.abs() / step.max(1e-3)).ceil() as usize).clamp(1, 4000);
    for i in 1..n {
        let t = theta1 + dtheta * i as f64 / n as f64;
        let (ct, st) = (t.cos(), t.sin());
        out.push((
            cx + rx * ct * cs - ry * st * sn,
            cy + rx * ct * sn + ry * st * cs,
        ));
    }
    out.push(p1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_commands() {
        let p = parse("M10 10 h 20 v20 H10 z m 5,5 l1-1", 0.2).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(
            p[0].pts,
            vec![(10.0, 10.0), (30.0, 10.0), (30.0, 30.0), (10.0, 30.0)]
        );
        assert!(p[0].closed);
        assert_eq!(p[1].pts, vec![(15.0, 15.0), (16.0, 14.0)]);
    }

    #[test]
    fn compact_numbers_and_arcs() {
        let p = parse("M0,0L.5.5-1e1 2A5 5 0 1010 0", 0.2).unwrap();
        assert_eq!(p[0].pts[1], (0.5, 0.5));
        assert_eq!(p[0].pts[2], (-10.0, 2.0));
        assert_eq!(*p[0].pts.last().unwrap(), (10.0, 0.0));
    }

    #[test]
    fn curves_end_on_target() {
        let p = parse(
            "M0 0 C 0 50 100 50 100 0 S 200 -50 200 0 Q 250 50 300 0 T 400 0",
            0.2,
        )
        .unwrap();
        assert_eq!(*p[0].pts.last().unwrap(), (400.0, 0.0));
        assert!(p[0].pts.len() > 20);
        assert!(parse("L 10 10", 0.2).is_err());
    }
}
