"""The repository's Cargo target flags, resolved as Cargo resolves them, for builds that must pass
them explicitly: through CARGO_ENCODED_RUSTFLAGS they replace inherited and user-level flags."""
import re
import subprocess
import tomllib

from .config import ROOT


def cfg_matches(key, cfg):
    """Evaluate a Cargo `cfg(...)` table key against `rustc --print cfg` lines; unknown syntax fails."""
    tokens = re.findall(r'"[^"]*"|\w+|[(),=]', key)
    position = 0

    def take(expected=None):
        nonlocal position
        if position == len(tokens) or expected not in (None, tokens[position]):
            raise ValueError(f"unsupported cfg key in .cargo/config.toml: {key}")
        position += 1
        return tokens[position - 1]

    def predicate():
        name = take()
        if name in ("all", "any", "not"):
            take("(")
            values = []
            while position < len(tokens) and tokens[position] != ")":
                values.append(predicate())
                if position < len(tokens) and tokens[position] == ",":
                    take(",")
            take(")")
            if name == "not" and len(values) != 1:
                raise ValueError(f"unsupported cfg key in .cargo/config.toml: {key}")
            return not values[0] if name == "not" else (all if name == "all" else any)(values)
        if position < len(tokens) and tokens[position] == "=":
            take("=")
            return f"{name}={take()}" in cfg
        return name in cfg

    take("cfg")
    take("(")
    matched = predicate()
    take(")")
    if position != len(tokens):
        raise ValueError(f"unsupported cfg key in .cargo/config.toml: {key}")
    return matched


def config_rustflags(config, triple, cfg):
    """The rustflags Cargo takes from `config` for `triple`: every matching target table, else build.rustflags."""
    flags = []
    for key, table in config.get("target", {}).items():
        if key == triple or (key.startswith("cfg(") and cfg_matches(key, cfg)):
            value = table.get("rustflags", [])
            flags += value.split() if isinstance(value, str) else value
    if not flags:
        value = config.get("build", {}).get("rustflags", [])
        flags = value.split() if isinstance(value, str) else value
    return list(flags)


def repository_rustflags(toolchain, triple, env):
    """The repository's `.cargo/config.toml` flags for `triple`, resolved here so that builds pass them
    explicitly and inherited RUSTFLAGS or a user-level Cargo configuration cannot reach the build."""
    config = tomllib.loads((ROOT / ".cargo/config.toml").read_text())
    cfg = subprocess.check_output(toolchain + ["rustc", "--print", "cfg", "--target", triple],
                                  cwd=ROOT, env=env, text=True).splitlines()
    return config_rustflags(config, triple, cfg)
