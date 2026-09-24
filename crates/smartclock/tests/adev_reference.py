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

taus, deviations, _, counts = allantools.oadev(
    phase, rate=1.0, data_type="phase", taus=TAUS
)
print(f"allantools {allantools.__version__}")
for tau, deviation, count in zip(taus, deviations, counts):
    print(f"({tau:.1f}, {float(deviation)!r}, {int(count)}),")
