//! Confidence intervals for the deviations, by NIST SP 1065.
//!
//! A sample variance is chi-squared distributed with some equivalent
//! number of degrees of freedom (section 5.3.2, equation 44), so the
//! interval on a deviation at one tau is (equation 45)
//!
//! ```text
//! sigma_min^2 = s^2 edf / chi2(p, edf)      sigma_max^2 = s^2 edf / chi2(1 - p, edf)
//! ```
//!
//! The edf depends on the estimator, the averaging factor, the number
//! of readings and the noise type at that tau (section 5.4).  The noise
//! type is found by the lag 1 autocorrelation method (sections 5.5.5
//! and 5.5.6): the tau-sampled phase is differenced until it is
//! stationary, and where its lag 1 autocorrelation then lands says
//! which power law dominates.  The edf is then Greenhall's combined
//! algorithm (section 5.4.1, "Uncertainty of Stability Variances Based
//! on Finite Differences", Greenhall and Riley 2003), the same one
//! allantools implements, against which every number here is tested.
//!
//! The chi-squared quantile is the Wilson-Hilferty approximation, which
//! puts the bounds within 0.4 % of the exact ones for two or more
//! degrees of freedom and within 2.6 % at one and a half; the test
//! records this against scipy.

/// The bounds of a confidence interval on one deviation.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Bounds {
    /// The deviation's lower bound.
    pub lower: f64,
    /// Its upper bound.
    pub upper: f64,
    /// The equivalent degrees of freedom the interval is from.
    pub edf: f64,
}

/// The confidence level of every interval: one sigma, 68.27 %.
///
/// The convention on stability plots, and the one SP 1065 section
/// 5.3.2 says most show.
pub const CONFIDENCE: f64 = 0.682_689_492_137_085_9;

/// The fewest tau-sampled readings the lag 1 method is applied to.
///
/// Section 5.5.6: "acceptable results can be obtained ... for N >= 32".
const NOISE_ID_MIN: usize = 32;

/// How many times the phase may be differenced before the noise type
/// is read anyway: two for a two-sample variance (section 5.5.6).
const NOISE_ID_MAX_DIFFERENCES: usize = 2;

/// The power law exponent of the noise dominating a tau-sampled phase
/// series, `alpha` in `S_y(f) ~ f^alpha`, from +2 (white PM) down to -2
/// (random walk FM), or `None` when the series is too short to tell.
///
/// `grid` is the phase at the full rate with holes; every `m`-th slot
/// is taken, as the handbook says to decimate phase for the tau of
/// interest, and the longest hole-free stretch of that is what the
/// method sees.  A quadratic trend is removed first -- frequency
/// offset and drift, the "deterministic components" section 5.5.6
/// says to remove -- as allantools does, so the two agree.
pub(super) fn noise_type(grid: &[Option<f64>], m: usize) -> Option<i32> {
    let decimated: Vec<Option<f64>> = grid.iter().step_by(m.max(1)).copied().collect();
    let longest = decimated
        .split(Option::is_none)
        .max_by_key(|run| run.len())?;
    if longest.len() < NOISE_ID_MIN {
        return None;
    }
    let mut z: Vec<f64> = detrended(&longest.iter().map(|x| x.unwrap_or(0.0)).collect::<Vec<_>>());
    let mut d = 0usize;
    loop {
        let r1 = lag1(&z);
        let rho = r1 / (1.0 + r1);
        if rho < 0.25 || d >= NOISE_ID_MAX_DIFFERENCES {
            // Section 5.5.6: p = -round(2 rho) - 2 d, and alpha = p + 2
            // for phase data.
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a rounded value between -1 and 5 fits an i32"
            )]
            let alpha = (-(2.0 * rho).round() - 2.0 * d as f64) as i32 + 2;
            return Some(alpha.clamp(-2, 2));
        }
        z = z.windows(2).map(|w| w[1] - w[0]).collect();
        d += 1;
    }
}

/// Lag 1 autocorrelation about the mean, section 5.5.6.
fn lag1(z: &[f64]) -> f64 {
    let mean = z.iter().sum::<f64>() / z.len() as f64;
    let numerator: f64 = z.windows(2).map(|w| (w[0] - mean) * (w[1] - mean)).sum();
    let denominator: f64 = z.iter().map(|x| (x - mean) * (x - mean)).sum();
    if denominator == 0.0 {
        0.0
    } else {
        numerator / denominator
    }
}

/// `z` with its least-squares quadratic in the index removed.
///
/// Fitted about the centre of the index range, where the normal
/// equations are best conditioned and the odd moments vanish.
fn detrended(z: &[f64]) -> Vec<f64> {
    let n = z.len() as f64;
    let centre = (n - 1.0) / 2.0;
    let t: Vec<f64> = (0..z.len()).map(|i| i as f64 - centre).collect();
    let s2: f64 = t.iter().map(|t| t * t).sum();
    let s4: f64 = t.iter().map(|t| t * t * t * t).sum();
    let sy: f64 = z.iter().sum();
    let sty: f64 = t.iter().zip(z).map(|(t, y)| t * y).sum();
    let st2y: f64 = t.iter().zip(z).map(|(t, y)| t * t * y).sum();
    // With the odd sums zero the system is [n s2; s2 s4] for (a, c)
    // and s2 alone for b.
    let det = n * s4 - s2 * s2;
    let (a, b, c) = if det == 0.0 || s2 == 0.0 {
        (sy / n, 0.0, 0.0)
    } else {
        (
            (s4 * sy - s2 * st2y) / det,
            sty / s2,
            (n * st2y - s2 * sy) / det,
        )
    };
    t.iter()
        .zip(z)
        .map(|(t, y)| y - (a + b * t + c * t * t))
        .collect()
}

/// Greenhall's equivalent degrees of freedom for a two-sample (Allan)
/// variance, overlapping, at averaging factor `m` over `n` phase
/// readings under noise `alpha`; `modified` for the modified form.
///
/// A direct port of the combined algorithm as SP 1065 section 5.4.1
/// cites it, in the cases a two-sample variance needs.  Returns `None`
/// where the record is too short for the estimator at all.
pub(super) fn edf(alpha: i32, m: usize, n: usize, modified: bool) -> Option<f64> {
    /// Differences taken: two, for the Allan variances.
    const D: usize = 2;
    /// Terms of the basic sum evaluated directly before the limiting
    /// form takes over.
    const J_MAX: f64 = 100.0;
    let d = D;
    let m_f = m as f64;
    // F: the phase filter, 1 for the modified form, m unmodified.
    // S: the stride, m for overlapping estimators.
    let f = if modified { 1.0 } else { m_f };
    let s = m_f;
    // L: the filter length applied to the phase.
    let l = m_f / f + m_f * d as f64;
    let big_m = 1.0 + ((s * (n as f64 - l)) / m_f).floor();
    if big_m < 1.0 {
        return None;
    }
    let j = big_m.min((d as f64 + 1.0) * s);
    let r = big_m / s;
    let inverse = if modified {
        // Case 1: modified variances, every alpha.
        if j <= J_MAX {
            basic_sum(j, big_m, s, 1.0, alpha, d) / (sz(0.0, 1.0, alpha, d).powi(2) * big_m)
        } else if r > d as f64 + 1.0 {
            let (a0, a1) = table1(alpha, d)?;
            (a0 - a1 / r) / r
        } else {
            let m_prime = J_MAX / r;
            basic_sum(J_MAX, J_MAX, m_prime, 1.0, alpha, d) / (sz(0.0, f, alpha, d).powi(2) * J_MAX)
        }
    } else if alpha <= 0 {
        // Case 2: unmodified variances, alpha <= 0.
        if j <= J_MAX {
            let m_prime = if m_f * (d as f64 + 1.0) <= J_MAX {
                m_f
            } else {
                f64::INFINITY
            };
            basic_sum(j, big_m, s, m_prime, alpha, d) / (sz(0.0, m_prime, alpha, d).powi(2) * big_m)
        } else if r > d as f64 + 1.0 {
            let (a0, a1) = table2(alpha, d)?;
            (a0 - a1 / r) / r
        } else {
            let m_prime = J_MAX / r;
            basic_sum(J_MAX, J_MAX, m_prime, f64::INFINITY, alpha, d)
                / (sz(0.0, f64::INFINITY, alpha, d).powi(2) * J_MAX)
        }
    } else if alpha == 1 {
        // Case 3: unmodified variances, flicker PM.
        if j <= J_MAX {
            basic_sum(j, big_m, s, m_f, 1, d) / (sz(0.0, m_f, 1, d).powi(2) * big_m)
        } else if r > d as f64 + 1.0 {
            let (a0, a1) = table2(alpha, d)?;
            let (b0, b1) = table3(d);
            (a0 - a1 / r) / ((b0 + b1 * m_f.ln()).powi(2) * r)
        } else {
            let m_prime = J_MAX / r;
            let (b0, b1) = table3(d);
            basic_sum(J_MAX, J_MAX, m_prime, m_prime, 1, d) / ((b0 + b1 * m_f.ln()).powi(2) * J_MAX)
        }
    } else {
        // Case 4: unmodified variances, white PM.
        if r.ceil() <= d as f64 {
            return None;
        }
        let a0 = binomial(4 * d, 2 * d) / binomial(2 * d, d).powi(2);
        let a1 = d as f64 / 2.0;
        (a0 - a1 / r) / big_m
    };
    (inverse.is_finite() && inverse > 0.0).then(|| 1.0 / inverse)
}

/// The basic sum of the algorithm: the generalized autocovariance of
/// the `d`-th difference summed over the lags that overlap.
fn basic_sum(j: f64, big_m: f64, s: f64, f: f64, alpha: i32, d: usize) -> f64 {
    let first = sz(0.0, f, alpha, d).powi(2);
    let second = (1.0 - j / big_m) * sz(j / s, f, alpha, d).powi(2);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "j is a whole number no larger than J_MAX"
    )]
    let terms = j as usize;
    let third: f64 = (1..terms)
        .map(|k| 2.0 * (1.0 - k as f64 / big_m) * sz(k as f64 / s, f, alpha, d).powi(2))
        .sum();
    first + second + third
}

/// Generalized autocovariance of the `d`-th difference of the filtered
/// phase.
fn sz(t: f64, f: f64, alpha: i32, d: usize) -> f64 {
    match d {
        1 => 2.0 * sx(t, f, alpha) - sx(t - 1.0, f, alpha) - sx(t + 1.0, f, alpha),
        2 => {
            6.0 * sx(t, f, alpha) - 4.0 * sx(t - 1.0, f, alpha) - 4.0 * sx(t + 1.0, f, alpha)
                + sx(t - 2.0, f, alpha)
                + sx(t + 2.0, f, alpha)
        }
        _ => unreachable!("only the two-sample variances are computed here"),
    }
}

/// Generalized autocovariance of the phase filtered over `f` readings.
fn sx(t: f64, f: f64, alpha: i32) -> f64 {
    if f.is_infinite() {
        return sw(t, alpha + 2);
    }
    f * f * (2.0 * sw(t, alpha) - sw(t - 1.0 / f, alpha) - sw(t + 1.0 / f, alpha))
}

/// Generalized autocovariance of the noise itself, per power law.
fn sw(t: f64, alpha: i32) -> f64 {
    let log_term = |power: i32| {
        if t == 0.0 {
            0.0
        } else {
            t.powi(power) * t.abs().ln()
        }
    };
    match alpha {
        2 => -t.abs(),
        1 => log_term(2),
        0 => t.abs().powi(3),
        -1 => log_term(4),
        -2 => t.abs().powi(5),
        -3 => log_term(6),
        -4 => t.abs().powi(7),
        _ => unreachable!("alpha is clamped to the tabulated range"),
    }
}

/// Limiting-form coefficients for the modified variances.
fn table1(alpha: i32, d: usize) -> Option<(f64, f64)> {
    let row: [Option<(f64, f64)>; 3] = match alpha {
        2 => [
            Some((2.0 / 3.0, 1.0 / 3.0)),
            Some((7.0 / 9.0, 1.0 / 2.0)),
            Some((22.0 / 25.0, 2.0 / 3.0)),
        ],
        1 => [
            Some((0.840, 0.345)),
            Some((0.997, 0.616)),
            Some((1.141, 0.843)),
        ],
        0 => [
            Some((1.079, 0.368)),
            Some((1.033, 0.607)),
            Some((1.184, 0.848)),
        ],
        -1 => [None, Some((1.048, 0.534)), Some((1.180, 0.816))],
        -2 => [None, Some((1.302, 0.535)), Some((1.175, 0.777))],
        _ => return None,
    };
    row.get(d - 1).copied().flatten()
}

/// Limiting-form coefficients for the unmodified variances.
fn table2(alpha: i32, d: usize) -> Option<(f64, f64)> {
    let row: [Option<(f64, f64)>; 3] = match alpha {
        2 => [
            Some((3.0 / 2.0, 1.0 / 2.0)),
            Some((35.0 / 18.0, 1.0)),
            Some((231.0 / 100.0, 3.0 / 2.0)),
        ],
        1 => [
            Some((78.6, 25.2)),
            Some((790.0, 410.0)),
            Some((9950.0, 6520.0)),
        ],
        0 => [
            Some((2.0 / 3.0, 1.0 / 6.0)),
            Some((2.0 / 3.0, 1.0 / 3.0)),
            Some((7.0 / 9.0, 1.0 / 2.0)),
        ],
        -1 => [None, Some((0.852, 0.375)), Some((0.997, 0.617))],
        -2 => [None, Some((1.079, 0.368)), Some((1.033, 0.607))],
        _ => return None,
    };
    row.get(d - 1).copied().flatten()
}

/// The flicker PM scale for the unmodified variances.
fn table3(d: usize) -> (f64, f64) {
    [(6.0, 4.0), (15.23, 12.0), (47.8, 40.0)][d - 1]
}

fn binomial(n: usize, k: usize) -> f64 {
    (1..=k).fold(1.0, |acc, i| acc * (n - k + i) as f64 / i as f64)
}

/// The chi-squared quantile at probability `p` for `edf` degrees of
/// freedom, by Wilson and Hilferty: the cube root of chi-squared over
/// its degrees of freedom is close to normal with mean `1 - 2/(9 edf)`
/// and variance `2/(9 edf)`.
fn chi_squared_quantile(p: f64, edf: f64) -> f64 {
    let z = normal_quantile(p);
    let spread = (2.0 / (9.0 * edf)).sqrt();
    edf * (1.0 - 2.0 / (9.0 * edf) + z * spread).powi(3)
}

/// The standard normal quantile, by bisection on the complementary
/// error function: a dozen lines that need no table and are exact to
/// the precision of `erfc`.
fn normal_quantile(p: f64) -> f64 {
    let cdf = |z: f64| 0.5 * erfc(-z / 2f64.sqrt());
    let (mut low, mut high) = (-10.0, 10.0);
    for _ in 0..200 {
        let mid = (low + high) / 2.0;
        if cdf(mid) < p {
            low = mid;
        } else {
            high = mid;
        }
    }
    (low + high) / 2.0
}

/// The complementary error function, by the continued fraction for
/// large arguments and the series for small, both to double precision.
fn erfc(x: f64) -> f64 {
    if x < 0.0 {
        return 2.0 - erfc(-x);
    }
    if x < 2.0 {
        // erf(x) = 2/sqrt(pi) sum (-1)^n x^(2n+1) / (n! (2n+1))
        let mut term = x;
        let mut sum = x;
        let mut n = 0.0;
        while term.abs() > 1e-17 * sum.abs() {
            n += 1.0;
            term *= -x * x / n;
            sum += term / (2.0 * n + 1.0);
        }
        return 1.0 - 2.0 / std::f64::consts::PI.sqrt() * sum;
    }
    // Lentz's method on the continued fraction
    // erfc(x) = exp(-x^2)/sqrt(pi) * 1/(x + 1/2/(x + 1/(x + 3/2/(x + ...))))
    let tiny = 1e-300;
    let mut f = x;
    let mut c = x;
    let mut d = 0.0;
    for k in 1..200 {
        let a = k as f64 / 2.0;
        d = x + a * d;
        d = if d == 0.0 { tiny } else { 1.0 / d };
        c = x + a / c;
        if c == 0.0 {
            c = tiny;
        }
        let delta = c * d;
        f *= delta;
        if (delta - 1.0).abs() < 1e-16 {
            break;
        }
    }
    (-x * x).exp() / (std::f64::consts::PI.sqrt() * f)
}

/// The interval on a deviation `s` with `edf` degrees of freedom, at
/// [`CONFIDENCE`]: SP 1065 equation 45.
pub(super) fn bounds(deviation: f64, edf: f64) -> Bounds {
    let p = (1.0 - CONFIDENCE) / 2.0;
    let variance = deviation * deviation;
    Bounds {
        lower: (variance * edf / chi_squared_quantile(1.0 - p, edf)).sqrt(),
        upper: (variance * edf / chi_squared_quantile(p, edf)).sqrt(),
        edf,
    }
}

#[cfg(test)]
mod tests {
    use super::CONFIDENCE;
    use super::chi_squared_quantile;
    use super::noise_type;

    /// `(edf, lower quantile, upper quantile)` at one sigma, from
    /// scipy.stats.chi2.ppf.
    const SCIPY: [(f64, f64, f64); 8] = [
        (1.5, 0.16064356749154324, 2.8706283705426463),
        (2.0, 0.3455075580468999, 3.6820432900185267),
        (3.0, 0.8338910101291227, 5.186262524204452),
        (5.0, 2.0559902692611867, 7.956394295315844),
        (10.0, 5.680617711186441, 14.325506344521887),
        (30.0, 22.341239802849884, 37.660761511376336),
        (100.0, 85.90532634828782, 114.09526869332386),
        (1000.0, 955.2935779834957, 1044.7064813006473),
    ];

    #[test]
    fn the_wilson_hilferty_quantile_is_within_half_a_percent_of_scipy_in_the_deviation() {
        let p = (1.0 - CONFIDENCE) / 2.0;
        for (edf, lower, upper) in SCIPY {
            for (want, got) in [
                (lower, chi_squared_quantile(p, edf)),
                (upper, chi_squared_quantile(1.0 - p, edf)),
            ] {
                // The bound on a deviation goes as the square root of
                // the quantile, so that is the error that matters.
                let error = ((want / got).sqrt() - 1.0).abs();
                let allowed = if edf < 2.0 { 0.03 } else { 0.005 };
                assert!(
                    error < allowed,
                    "edf {edf}: {got} against {want}, deviation error {error}"
                );
            }
        }
    }

    /// Park and Miller's generator, as the reference test uses it.
    fn uniform(n: usize) -> Vec<f64> {
        let mut state: u64 = 1_234_567_890;
        (0..n)
            .map(|_| {
                state = (16_807 * state) % 2_147_483_647;
                state as f64 / 2_147_483_647.0 - 0.5
            })
            .collect()
    }

    #[test]
    fn white_phase_noise_reads_as_alpha_two_and_its_random_walk_as_zero() {
        let white: Vec<Option<f64>> = uniform(1000).into_iter().map(Some).collect();
        assert_eq!(noise_type(&white, 1), Some(2));
        let walk: Vec<Option<f64>> = uniform(1000)
            .into_iter()
            .scan(0.0, |x, step| {
                *x += step;
                Some(Some(*x))
            })
            .collect();
        assert_eq!(noise_type(&walk, 1), Some(0));
        assert_eq!(noise_type(&walk[..20], 1), None, "too short to tell");
    }
}
