"""Paired bootstrap statistics and fail-closed cell acceptance."""
import math
import random
import statistics


def quantile(values, probability):
    ordered = sorted(values)
    if not ordered:
        raise ValueError("empty sample")
    position = (len(ordered) - 1) * probability
    lo, hi = math.floor(position), math.ceil(position)
    return ordered[lo] + (ordered[hi] - ordered[lo]) * (position - lo)


def _positive_finite(value):
    try:
        return type(value) in (int, float) and math.isfinite(value) and value > 0
    except OverflowError:
        return False


def _positive_ratio(numerator, denominator):
    try:
        value = numerator / denominator
    except (OverflowError, ZeroDivisionError) as error:
        raise ValueError("derived ratios must be finite and positive") from error
    if not _positive_finite(value):
        raise ValueError("derived ratios must be finite and positive")
    return value


def ratio_bounds(target, baseline, resamples=5000, confidence=0.95, seed=42017):
    """Paired percentile bootstrap estimates, resampling complete randomized pairs.

    Confidence applies separately to each one-sided bound, not jointly to the
    two endpoints. These estimates do not guarantee coverage for arbitrary
    sample sizes, distributions or temporally dependent measurements.
    """
    if type(resamples) is not int or resamples <= 0:
        raise ValueError("resamples must be a positive integer")
    if type(confidence) not in (int, float) or not 0 < confidence < 1 or not math.isfinite(confidence):
        raise ValueError("confidence must be finite and strictly between zero and one")
    if len(target) != len(baseline) or len(target) < 2:
        raise ValueError("at least two complete pairs required")
    if not all(_positive_finite(x) for values in (target, baseline) for x in values):
        raise ValueError("samples must be finite and positive")
    rng, count = random.Random(seed), len(target)
    bounds = {"median": [], "p95": []}
    for _ in range(resamples):
        indices = rng.choices(range(count), k=count)
        a, b = [target[i] for i in indices], [baseline[i] for i in indices]
        bounds["median"].append(_positive_ratio(statistics.median(a), statistics.median(b)))
        bounds["p95"].append(_positive_ratio(quantile(a, 0.95), quantile(b, 0.95)))
    result = {
        "median_ratio": _positive_ratio(statistics.median(target), statistics.median(baseline)),
        "p95_ratio": _positive_ratio(quantile(target, 0.95), quantile(baseline, 0.95)),
        "median_upper": quantile(bounds["median"], confidence),
        "p95_upper": quantile(bounds["p95"], confidence),
        "median_lower": quantile(bounds["median"], 1 - confidence),
        "p95_lower": quantile(bounds["p95"], 1 - confidence),
    }
    if not all(_positive_finite(value) for value in result.values()):
        raise ValueError("derived bounds must be finite and positive")
    return result


def evaluate_cell(cell, policy):
    reasons = []
    samples = cell.get("samples", [])
    if len(samples) < policy["minimum_samples"]:
        reasons.append("insufficient samples")
    if cell.get("status") != "measured":
        reasons.append("cell not measured")
    oracle = cell.get("oracle_pixels_sha256")
    if not isinstance(oracle, str) or len(oracle) != 64:
        reasons.append("missing oracle")
    if len({s.get("pair") for s in samples}) != len(samples):
        reasons.append("duplicate pair IDs")
    for sample in samples:
        if sample.get("order") not in (["pagura", cell.get("engine")], [cell.get("engine"), "pagura"]):
            reasons.append("invalid paired order")
            break
        for key in ("pagura", "baseline"):
            run = sample.get(key, {})
            if run.get("status") != "ok" or run.get("exit_code") != 0 or run.get("pixels_sha256") != oracle:
                reasons.append("incorrect, failed or missing output")
            if not _positive_finite(run.get("elapsed_ns")):
                reasons.append("invalid timing")
            if type(run.get("peak_rss_bytes")) is not int or run["peak_rss_bytes"] <= 0:
                reasons.append("missing peak RSS")
    metrics = {}
    if not reasons:
        try:
            metrics = ratio_bounds([s["pagura"]["elapsed_ns"] for s in samples],
                                   [s["baseline"]["elapsed_ns"] for s in samples],
                                   policy["bootstrap_resamples"], policy["confidence"])
            metrics["peak_rss_ratio"] = _positive_ratio(
                max(s["pagura"]["peak_rss_bytes"] for s in samples),
                max(s["baseline"]["peak_rss_bytes"] for s in samples))
        except ValueError as error:
            metrics = {}
            reasons.append(f"invalid statistics: {error}")
        else:
            for statistic in policy["statistics"]:
                if metrics[statistic + "_upper"] > policy["latency_ratio_max"]:
                    reasons.append(f"{statistic} confidence bound misses 20% latency target")
            if metrics["peak_rss_ratio"] > policy["peak_rss_ratio_max"]:
                reasons.append("peak memory regression")
    return {"pass": not reasons, "reasons": sorted(set(reasons)), "metrics": metrics}


def comparison_statistics(cell, policy, metric="elapsed_ns"):
    """Exploratory paired statistics; unverified pixels never become gate evidence.

    Keep all pairs: any output/timer failure suppresses a ratio instead of selecting
    only favourable pairs. Missing RSS is reported as missing, never zero.
    """
    samples = cell.get("samples", [])
    if len(samples) < 2:
        return {"status": "insufficient-pairs"}
    values = {key: [] for key in ("pagura", "baseline")}
    for sample in samples:
        for key in values:
            run = sample.get(key, {})
            if run.get("status") not in ("ok", "unverified"):
                return {"status": "failed-output"}
            value = run.get(metric) if metric == "elapsed_ns" else (run.get("stages") or {}).get(metric)
            if type(value) is not int or value <= 0:
                return {"status": "unavailable-timing"}
            values[key].append(value)
    try:
        metrics = ratio_bounds(values["pagura"], values["baseline"],
                               policy["bootstrap_resamples"], policy["confidence"])
        rss = [[sample[key].get("peak_rss_bytes") for sample in samples] for key in values]
        metrics["peak_rss_ratio"] = (_positive_ratio(max(rss[0]), max(rss[1])) if all(
            type(value) is int and value > 0 for group in rss for value in group) else None)
    except ValueError as error:
        return {"status": "invalid-statistics", "reason": str(error)}
    for key in values:
        metrics[key + "_median_ns"] = statistics.median(values[key])
        metrics[key + "_p95_ns"] = quantile(values[key], 0.95)
    return {"status": "measured", **metrics}

