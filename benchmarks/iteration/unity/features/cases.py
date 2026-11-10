"""Explicit immutable inputs and backend-specific budgets for feature fixtures."""
import json

NAMES = ("query_filter", "dirty_epoch", "archetype_churn", "dependency_dag", "buffer_fixed", "buffer_resize")


def name(args):
    return getattr(args, "case", "query_filter")


def extended(args):
    return name(args) != "query_filter"


def config(args):
    return {"percent": args.percent, "distribution": args.distribution, "length": args.length,
            "trim": args.trim, "fork_join": args.topology == "fork-join"}


def variant(args):
    case = name(args)
    if case == "query_filter": return f"a{args.archetypes}"
    if case == "dirty_epoch": return f"p{args.percent}_{args.distribution}"
    if case == "archetype_churn": return f"p{args.percent}"
    if case == "dependency_dag": return args.topology
    if case == "buffer_fixed": return f"length{args.length}"
    return f"p{args.percent}_{'trim' if args.trim else 'reuse'}"


def workers(args):
    if args.runners == 1: return 0
    # A Send-only retained Schedule does not tick the shared executor on its caller.
    return args.runners if name(args) in ("dirty_epoch", "dependency_dag") else args.runners - 1


def encoded_config(args):
    return (json.dumps(config(args), sort_keys=True, separators=(",", ":")) + "\n").encode()
