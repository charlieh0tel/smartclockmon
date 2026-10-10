//! The dynamics `docs/loop.html` states for the disciplining loop --
//! its closed-loop poles, the slow pole's time constant, the overshoot
//! -- recomputed from the update law in `docs/firmware/loop.md`, so a
//! figure on the page cannot drift from the arithmetic behind it.

use std::fs;

/// Seconds between updates.
const PERIOD: f64 = 10.0;

/// The prefilter's numerator: a = `PREFILTER` / τ.
const PREFILTER: f64 = 29.75;

/// The loop time constants the page's by-model table holds, seconds.
const TAUS: [f64; 3] = [500.0, 700.0, 1000.0];

/// The time constant the page's poles are drawn for, seconds.
const DRAWN: f64 = 500.0;

/// Updates simulated for a phase step: long enough to settle at the
/// longest τ.
const STEPS: usize = 3000;

/// A complex number, enough of one for the roots of a cubic.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    fn add(self, other: Self) -> Self {
        Self::new(self.re + other.re, self.im + other.im)
    }
    fn sub(self, other: Self) -> Self {
        Self::new(self.re - other.re, self.im - other.im)
    }
    fn mul(self, other: Self) -> Self {
        Self::new(
            self.re * other.re - self.im * other.im,
            self.re * other.im + self.im * other.re,
        )
    }
    fn div(self, other: Self) -> Self {
        let norm = other.re * other.re + other.im * other.im;
        Self::new(
            (self.re * other.re + self.im * other.im) / norm,
            (self.im * other.re - self.re * other.im) / norm,
        )
    }
    fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
    fn ln(self) -> Self {
        Self::new(self.abs().ln(), self.im.atan2(self.re))
    }
}

/// The roots of z³ + b·z² + c·z + d, by Durand-Kerner iteration.
fn cubic_roots(b: f64, c: f64, d: f64) -> [Complex; 3] {
    let value = |z: Complex| {
        let real = |x: f64| Complex::new(x, 0.0);
        z.mul(z)
            .mul(z)
            .add(real(b).mul(z).mul(z))
            .add(real(c).mul(z))
            .add(real(d))
    };
    let seed = Complex::new(0.4, 0.9);
    let mut roots = [Complex::new(1.0, 0.0), seed, seed.mul(seed)];
    for _ in 0..500 {
        for i in 0..3 {
            let mut denominator = Complex::new(1.0, 0.0);
            for j in 0..3 {
                if i != j {
                    denominator = denominator.mul(roots[i].sub(roots[j]));
                }
            }
            roots[i] = roots[i].sub(value(roots[i]).div(denominator));
        }
    }
    roots
}

/// One update as a matrix on the state (phase error, filtered error,
/// integrator), with G taken as 1 since it cancels: f ← (1 − a)·f + a·x,
/// I ← I + 10·k·f, u = K·f + I, and the oscillator moves the phase by
/// −10·u over the ten seconds.
fn update(tau: f64) -> [[f64; 3]; 3] {
    let a = PREFILTER / tau;
    let proportional = 1.0 / tau;
    let integral = 1.0 / (4.0 * tau * tau);
    let filter = [a, 1.0 - a, 0.0];
    let integrator = [
        PERIOD * integral * filter[0],
        PERIOD * integral * filter[1],
        1.0 + PERIOD * integral * filter[2],
    ];
    let drive: Vec<f64> = (0..3)
        .map(|i| proportional * filter[i] + integrator[i])
        .collect();
    let phase = [
        1.0 - PERIOD * drive[0],
        -PERIOD * drive[1],
        -PERIOD * drive[2],
    ];
    [phase, filter, integrator]
}

/// The update's poles, mapped to the s-plane and scaled by τ, slowest
/// first.
fn poles(tau: f64) -> Vec<Complex> {
    let m = update(tau);
    let trace = m[0][0] + m[1][1] + m[2][2];
    let minors = m[0][0] * m[1][1] - m[0][1] * m[1][0] + m[0][0] * m[2][2] - m[0][2] * m[2][0]
        + m[1][1] * m[2][2]
        - m[1][2] * m[2][1];
    let determinant = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let mut out: Vec<Complex> = cubic_roots(-trace, minors, -determinant)
        .iter()
        .map(|z| {
            let s = z.ln();
            Complex::new(s.re * tau / PERIOD, s.im * tau / PERIOD)
        })
        .collect();
    out.sort_by(|x, y| y.re.total_cmp(&x.re));
    out
}

/// The phase error after each update, from a unit error at rest.
fn step(tau: f64) -> Vec<f64> {
    let m = update(tau);
    let mut state = [1.0, 0.0, 0.0];
    (0..STEPS)
        .map(|_| {
            state = [0, 1, 2].map(|row| (0..3).map(|col| m[row][col] * state[col]).sum());
            state[0]
        })
        .collect()
}

fn page() -> String {
    fs::read_to_string(format!(
        "{}/../../docs/loop.html",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("the page")
}

/// A pole written as the page writes it: "−0.37" or "(−1.35 ± 0.51j)".
fn written(pole: Complex) -> String {
    if pole.im.abs() < 1e-9 {
        format!("−{:.2}", -pole.re)
    } else {
        format!("(−{:.2} ± {:.2}j)", -pole.re, pole.im.abs())
    }
}

#[test]
fn the_page_states_the_poles_of_the_update() {
    let page = page();
    let drawn = poles(DRAWN);
    for pole in [drawn[0], drawn[1]] {
        let text = format!("{}/τ", written(pole));
        assert!(page.contains(&text), "loop.html does not state {text}");
    }
    let pair = drawn[1];
    let damping = -pair.re / pair.abs();
    let text = format!("damping ratio {damping:.2}");
    assert!(page.contains(&text), "loop.html does not state {text}");
    let worst = TAUS
        .iter()
        .flat_map(|&tau| poles(tau).into_iter().zip(drawn.clone()))
        .map(|(pole, reference)| {
            let moved = Complex::new(pole.re - reference.re, pole.im.abs() - reference.im.abs());
            moved.abs() / reference.abs()
        })
        .fold(0.0, f64::max);
    assert!(
        worst < 0.06 && page.contains("within 6 %"),
        "the poles move {:.1} % across the table's time constants",
        worst * 100.0
    );
}

#[test]
fn the_page_states_the_continuous_cubic_s_roots() {
    let p = PREFILTER / PERIOD;
    let roots = cubic_roots(p, p, p / 4.0);
    let mut roots: Vec<Complex> = roots.to_vec();
    roots.sort_by(|x, y| y.re.total_cmp(&x.re));
    let page = page();
    for root in [roots[0], roots[1]] {
        let text = format!("{}/τ", written(root));
        assert!(page.contains(&text), "loop.html does not state {text}");
    }
}

#[test]
fn the_page_states_each_time_constant_s_slow_pole_and_the_overshoot() {
    let page = page();
    for tau in TAUS {
        let slowest = tau / -poles(tau)[0].re;
        let text = format!("{} s", (slowest / 10.0).round() * 10.0);
        assert!(
            page.contains(&text),
            "loop.html does not state {text} for τ = {tau} s"
        );
        let response = step(tau);
        let overshoot = -response.iter().copied().fold(f64::INFINITY, f64::min);
        assert!(
            ((overshoot * 100.0 / 5.0).round() * 5.0 - 20.0).abs() < f64::EPSILON,
            "a phase step overshoots by {:.1} % at τ = {tau} s",
            overshoot * 100.0
        );
        let reached = response
            .iter()
            .position(|x| x.abs() < (-1.0f64).exp())
            .expect("the error falls to 1/e");
        let seconds = (reached + 1) as f64 * PERIOD;
        assert!(
            (seconds / tau - 1.0).abs() < 0.1,
            "the error falls to 1/e after {seconds} s at τ = {tau} s"
        );
    }
    assert!(page.contains("overshoots by about 20 %"));
    assert!(page.contains("63 % of it in about τ"));
}
