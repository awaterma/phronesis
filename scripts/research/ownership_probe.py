#!/usr/bin/env python3
"""Read-only, bounded ownership-shape probe for the BORIS research sample.

BORIS: Christian Schott, "Visualizing Ownership and Borrowing in Rust
Programs", master's thesis, Universitaet Wuerzburg, 2024.

This is deliberately not a borrow checker. It asks rust-analyzer for a lossless
syntax tree, extracts reviewable source events, and evaluates five named
downstream-consumer fixtures. Semantic confirmation belongs in the accompanying research
report; this script never runs Cargo and never writes to the target repository.
"""

from __future__ import annotations

import argparse
import json
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable


@dataclass(frozen=True)
class Case:
    name: str
    relative_file: str
    function: str
    expected_shape: str


CASES = (
    Case(
        "filter-before-clone",
        "src/game_core/initialize_runtime.rs",
        "execute_rete_with_provenance",
        "filter_before_clone",
    ),
    Case(
        "snapshot-before-mutation-await",
        "src/game_core/secret_reveal.rs",
        "check_secrets_on_arrival",
        "snapshot_before_await",
    ),
    Case(
        "read-write-phase-separation",
        "src/game_core/spatial_state.rs",
        "reposition_party_member",
        "read_before_get_mut",
    ),
    Case(
        "small-current-location-clone",
        "src/game_core/reckless_actions.rs",
        "handle_reckless_action_check",
        "current_location_clone",
    ),
    Case(
        "arc-lock-async-service",
        "src/llm/scheduler.rs",
        "acquire",
        "locks_end_before_await",
    ),
)


def walk(node: dict[str, Any]) -> Iterable[dict[str, Any]]:
    yield node
    for child in node.get("children", []):
        yield from walk(child)


def source_slice(source: bytes, node: dict[str, Any]) -> str:
    return source[node["start"][0] : node["end"][0]].decode("utf-8")


def line(node: dict[str, Any]) -> int:
    return int(node["start"][1]) + 1


def function_name(node: dict[str, Any]) -> str | None:
    for child in node.get("children", []):
        if child.get("kind") == "NAME":
            for nested in walk(child):
                if nested.get("kind") == "IDENT":
                    return nested.get("text")
    return None


def parse(source: str) -> dict[str, Any]:
    result = subprocess.run(
        ["rust-analyzer", "parse", "--json"],
        input=source,
        text=True,
        capture_output=True,
        check=True,
    )
    return json.loads(result.stdout)


def find_function(tree: dict[str, Any], name: str) -> dict[str, Any]:
    matches = [
        node
        for node in walk(tree)
        if node.get("kind") == "FN" and function_name(node) == name
    ]
    if len(matches) != 1:
        raise ValueError(f"expected one function {name!r}, found {len(matches)}")
    return matches[0]


def nodes_of_kind(function: dict[str, Any], kind: str) -> list[dict[str, Any]]:
    return [node for node in walk(function) if node.get("kind") == kind]


def method_name(node: dict[str, Any]) -> str | None:
    direct_names = [
        child
        for child in node.get("children", [])
        if child.get("kind") == "NAME_REF"
    ]
    if not direct_names:
        return None
    identifiers = [
        child.get("text")
        for child in walk(direct_names[-1])
        if child.get("kind") == "IDENT"
    ]
    return identifiers[-1] if identifiers else None


def method_events(function: dict[str, Any], source: bytes) -> list[dict[str, Any]]:
    events = []
    for node in nodes_of_kind(function, "METHOD_CALL_EXPR"):
        name = method_name(node)
        if name in {
            "clone",
            "cloned",
            "collect",
            "filter",
            "get_mut",
            "lock",
            "to_owned",
            "to_string",
        }:
            events.append(
                {
                    "kind": "method_call",
                    "operation": name,
                    "line": line(node),
                    "span": [node["start"][0], node["end"][0]],
                    "source": source_slice(source, node).replace("\n", " ").strip(),
                }
            )
    return events


def lock_scope_events(function: dict[str, Any], source: bytes) -> list[dict[str, Any]]:
    events = []
    for block in nodes_of_kind(function, "BLOCK_EXPR"):
        lock_nodes = [
            node
            for node in walk(block)
            if node.get("kind") == "METHOD_CALL_EXPR" and method_name(node) == "lock"
        ]
        if not lock_nodes:
            continue
        # Keep only the narrowest block that owns each lock call.
        for lock in lock_nodes:
            nested_blocks = [
                child
                for child in nodes_of_kind(block, "BLOCK_EXPR")
                if child is not block
                and child["start"][0] <= lock["start"][0] < child["end"][0]
            ]
            if nested_blocks:
                continue
            events.append(
                {
                    "kind": "lock_scope",
                    "line": line(lock),
                    "scope_end_line": int(block["end"][1]) + 1,
                    "source": source_slice(source, lock).replace("\n", " ").strip(),
                }
            )
    return events


def analyze_case(root: Path, case: Case) -> dict[str, Any]:
    path = root / case.relative_file
    source = path.read_bytes()
    source_text = source.decode("utf-8")
    function = find_function(parse(source_text), case.function)
    methods = method_events(function, source)
    awaits = [
        {"kind": "await", "line": line(node)}
        for node in nodes_of_kind(function, "AWAIT_EXPR")
    ]
    locks = lock_scope_events(function, source)
    operation_lines: dict[str, list[int]] = {}
    for event in methods:
        operation_lines.setdefault(event["operation"], []).append(event["line"])

    if case.expected_shape == "filter_before_clone":
        filter_ends = [
            event["span"][1]
            for event in methods
            if event["operation"] == "filter"
        ]
        clone_ends = [
            event["span"][1]
            for event in methods
            if event["operation"] == "cloned"
        ]
        verdict = bool(filter_ends and clone_ends) and min(filter_ends) < min(clone_ends)
    elif case.expected_shape == "snapshot_before_await":
        verdict = bool(operation_lines.get("collect") and awaits) and min(
            operation_lines["collect"]
        ) < min(event["line"] for event in awaits)
    elif case.expected_shape == "read_before_get_mut":
        text = source_slice(source, function)
        verdict = "let in_combat" in text and bool(operation_lines.get("get_mut")) and text.index(
            "let in_combat"
        ) < text.index(".get_mut(")
    elif case.expected_shape == "current_location_clone":
        verdict = any(
            event["operation"] == "clone" and "current_location" in event["source"]
            for event in methods
        )
    elif case.expected_shape == "locks_end_before_await":
        first_await = min((event["line"] for event in awaits), default=-1)
        verdict = bool(locks and first_await > 0) and all(
            event["scope_end_line"] < first_await for event in locks
        )
    else:
        raise AssertionError(case.expected_shape)

    return {
        "case": case.name,
        "file": case.relative_file,
        "function": case.function,
        "ast_shape": case.expected_shape,
        "ast_shape_observed": verdict,
        "events": sorted(methods + awaits + locks, key=lambda event: event["line"]),
        "limits": [
            "syntax only: receiver and cloned-value types are unresolved",
            "does not prove moves, borrow liveness, allocation cost, or runtime frequency",
            "macro-expanded and compiler-generated operations are absent",
        ],
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path, help="read-only downstream-consumer checkout")
    parser.add_argument("--pretty", action="store_true")
    args = parser.parse_args()
    root = args.root.resolve()
    results = [analyze_case(root, case) for case in CASES]
    print(json.dumps({"root": str(root), "cases": results}, indent=2 if args.pretty else None))


if __name__ == "__main__":
    main()
