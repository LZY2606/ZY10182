#!/usr/bin/env python3
"""Fixed fixture generator for 伏安峰议台.

Waveform: 2 CV cycles (up/down/up/down) with holding plateaus at every
reversal. Two partially overlapping Gaussians per scan direction simulate
the redox peaks. cv_sce is the same run reported against a different
reference electrode convention. cv_plateau_ext extends the first reversal
plateau by one sample for the no-jitter segmentation check.
"""
import math
import os

STEP = 0.01
E_LO, E_HI = -0.20, 0.80
PLAT = 5          # initial / final
REV_PLAT = 4      # reversal plateaus

ANO_PEAKS = [(0.30, 8e-6, 0.020), (0.36, 6e-6, 0.025)]
CAT_PEAKS = [(0.27, -7e-6, 0.025), (0.33, -5e-6, 0.030)]


def ramp(e0, e1):
    n = int(round(abs(e1 - e0) / STEP))
    return [round(e0 + math.copysign(STEP, e1 - e0) * i, 5) for i in range(n + 1)]


def potential_trace(extra_first_plateau=False):
    seq = [0.0] * PLAT
    seq += ramp(0.0, E_HI)                       # up cycle 1 (includes 0.0 and 0.8)
    p1 = REV_PLAT + (1 if extra_first_plateau else 0)
    seq += [E_HI] * p1
    seq += ramp(E_HI, E_LO)[1:]                  # down (drop shared apex)
    seq += [E_LO] * REV_PLAT
    seq += ramp(E_LO, E_HI)[1:]                  # up cycle 2
    seq += [E_HI] * REV_PLAT
    seq += ramp(E_HI, 0.0)[1:]                   # down to start
    seq += [0.0] * PLAT
    return seq


def current_at(e, direction, v):
    cap = 20e-6 * v * direction                  # capacitive background
    offset = 0.1e-6 + 1e-6 * e                   # ohmic-ish slope
    peaks = ANO_PEAKS if direction > 0 else CAT_PEAKS
    scale = math.sqrt(v / 0.1)
    pk = sum(a * scale * math.exp(-0.5 * ((e - mu) / w) ** 2) for mu, a, w in peaks)
    return cap + offset + pk


def write(path, name, ref, v, area, shift=0.0, extra_first_plateau=False):
    dt = STEP / v
    pot = potential_trace(extra_first_plateau)
    rows = []
    for i, e in enumerate(pot):
        if i == 0:
            direction = 1
        else:
            d = e - pot[i - 1]
            direction = 1 if d > 0 else (-1 if d < 0 else direction)
        rows.append((i * dt, e + shift, current_at(e + shift, direction, v)))
    with open(path, "w") as f:
        f.write(f"# name: {name}\n")
        f.write(f"# reference_electrode: {ref}\n")
        f.write("# potential_unit: V\n# current_unit: A\n# time_unit: s\n")
        f.write(f"# scan_rate: {v}\n# scan_rate_unit: V/s\n")
        f.write(f"# electrode_area: {area}\n# electrode_area_unit: cm^2\n")
        f.write("time_s,potential_v,current_a\n")
        for t, e, i in rows:
            f.write(f"{t:.4f},{e:.5f},{i:.9e}\n")
    return len(rows)


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    n1 = write(os.path.join(here, "cv_main.csv"), "cv_main 0.1 V/s vs Ag/AgCl",
               "Ag/AgCl (sat. KCl)", 0.1, 0.0707)
    n2 = write(os.path.join(here, "cv_fast.csv"), "cv_fast 0.4 V/s vs Ag/AgCl",
               "Ag/AgCl (sat. KCl)", 0.4, 0.0707)
    n3 = write(os.path.join(here, "cv_sce.csv"), "cv_sce 0.1 V/s vs SCE",
               "SCE", 0.1, 0.0707, shift=-0.045)
    n4 = write(os.path.join(here, "cv_plateau_ext.csv"),
               "cv_main 0.1 V/s vs Ag/AgCl (reversal plateau +1)",
               "Ag/AgCl (sat. KCl)", 0.1, 0.0707, extra_first_plateau=True)
    print(f"wrote {n1}, {n2}, {n3}, {n4} rows")


if __name__ == "__main__":
    main()
