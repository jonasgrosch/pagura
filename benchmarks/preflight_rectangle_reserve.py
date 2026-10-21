#!/usr/bin/env python3
"""Reuse the all76-output preflight without changing its historical defaults."""
import rectangle_reserve as experiment
from preflight_color_reuse import main

if __name__ == "__main__":
    raise SystemExit(main(experiment_module=experiment, experiment_name="rectangle-reserve",
                          entrypoint=__file__))
