"""Reference values for tests/adev_reference.rs, from allantools.

Run with allantools installed (2024.06 produced the values in the
test):

    python3 crates/smartclock/tests/adev_reference.py

The phase is built exactly as the Rust test builds it, so the two see
the same numbers.
"""

import allantools

READINGS = 1000
TAUS = [1, 2, 5, 10, 20, 50, 100, 200]

# Park and Miller's minimal standard generator, integers only, so both
# languages produce the same sequence.
phase = [0.0]
n = 1234567890
for _ in range(READINGS - 1):
    n = (16807 * n) % 2147483647
    phase.append(phase[-1] + (n / 2147483647 - 0.5) * 1e-9)

print(f"allantools {allantools.__version__}")
taus, deviations, _, counts = allantools.oadev(
    phase, rate=1.0, data_type="phase", taus=TAUS
)
print("oadev")
for tau, deviation, count in zip(taus, deviations, counts):
    print(f"({tau:.1f}, {float(deviation)!r}, {int(count)}),")

# The modified and time deviations share a count: TDEV is MDEV scaled.
taus, modified, _, counts = allantools.mdev(
    phase, rate=1.0, data_type="phase", taus=TAUS
)
_, times, _, _ = allantools.tdev(phase, rate=1.0, data_type="phase", taus=TAUS)
print("mdev, tdev")
for tau, deviation, time, count in zip(taus, modified, times, counts):
    print(f"({tau:.1f}, {float(deviation)!r}, {float(time)!r}, {int(count)}),")

taus, peaks, _, counts = allantools.mtie(
    phase, rate=1.0, data_type="phase", taus=TAUS
)
print("mtie")
for tau, peak, count in zip(taus, peaks, counts):
    print(f"({tau:.1f}, {float(peak)!r}, {int(count)}),")

# The intervals: noise type by lag 1 autocorrelation, Greenhall edf,
# chi-squared bounds at one sigma.  Past the point where the decimated
# record is under 30 readings allantools refuses to identify the noise;
# the last answer is carried forward, as the estimator does.
from allantools import ci

taus, deviations, _, _ = allantools.oadev(phase, rate=1.0, data_type="phase", taus=TAUS)
_, modified, _, _ = allantools.mdev(phase, rate=1.0, data_type="phase", taus=TAUS)
print("confidence")
carried = None
for tau, deviation, mod in zip(taus, deviations, modified):
    m = int(tau)
    try:
        alpha, _, _, _ = ci.autocorr_noise_id(phase, m, data_type="phase", dmin=0, dmax=2)
        carried = alpha
    except NotImplementedError:
        alpha = carried
    edf = ci.edf_greenhall(alpha, 2, m, READINGS, overlapping=True, modified=False)
    edf_mod = ci.edf_greenhall(alpha, 2, m, READINGS, overlapping=True, modified=True)
    low, high = ci.confidence_interval(deviation, edf)
    low_mod, high_mod = ci.confidence_interval(mod, edf_mod)
    print(
        f"({tau:.1f}, {alpha}, {float(edf)!r}, {float(low)!r}, {float(high)!r}, "
        f"{float(edf_mod)!r}, {float(low_mod)!r}, {float(high_mod)!r}),"
    )
