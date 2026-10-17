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


def ratio_bounds(target, baseline, resamples=5000, confidence=0.95, seed=42017):
    """Paired nonparametric percentile bootstrap; resample complete randomized pairs."""
    if len(target) != len(baseline) or len(target) < 2:
        raise ValueError("at least two complete pairs required")
    if any(not math.isfinite(x) or x <= 0 for x in target + baseline):
        raise ValueError("samples must be finite and positive")
    rng, count = random.Random(seed), len(target)
    bounds = {"median": [], "p95": []}
    for _ in range(resamples):
        indices = rng.choices(range(count), k=count)
        a, b = [target[i] for i in indices], [baseline[i] for i in indices]
        bounds["median"].append(statistics.median(a) / statistics.median(b))
        bounds["p95"].append(quantile(a, 0.95) / quantile(b, 0.95))
    return {
        "median_ratio": statistics.median(target) / statistics.median(baseline),
        "p95_ratio": quantile(target, 0.95) / quantile(baseline, 0.95),
        "median_upper": quantile(bounds["median"], confidence),
        "p95_upper": quantile(bounds["p95"], confidence),
    }


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
            if not isinstance(run.get("elapsed_ns"), (int, float)) or not math.isfinite(run["elapsed_ns"]) or run["elapsed_ns"] <= 0:
                reasons.append("invalid timing")
            if not isinstance(run.get("peak_rss_bytes"), int) or run["peak_rss_bytes"] <= 0:
                reasons.append("missing peak RSS")
    metrics = {}
    if not reasons:
        metrics = ratio_bounds([s["pagura"]["elapsed_ns"] for s in samples],
                               [s["baseline"]["elapsed_ns"] for s in samples],
                               policy["bootstrap_resamples"], policy["confidence"])
        for statistic in policy["statistics"]:
            if metrics[statistic + "_upper"] > policy["latency_ratio_max"]:
                reasons.append(f"{statistic} confidence bound misses 20% latency target")
        metrics["peak_rss_ratio"] = max(s["pagura"]["peak_rss_bytes"] for s in samples) / max(s["baseline"]["peak_rss_bytes"] for s in samples)
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
    metrics = ratio_bounds(values["pagura"], values["baseline"],
                           policy["bootstrap_resamples"], policy["confidence"])
    for key in values:
        metrics[key + "_median_ns"] = statistics.median(values[key])
        metrics[key + "_p95_ns"] = quantile(values[key], 0.95)
    rss = [[sample[key].get("peak_rss_bytes") for sample in samples] for key in values]
    metrics["peak_rss_ratio"] = (max(rss[0]) / max(rss[1]) if all(
        type(value) is int and value > 0 for group in rss for value in group) else None)
    return {"status": "measured", **metrics}

