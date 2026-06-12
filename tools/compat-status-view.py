#!/usr/bin/env python3
"""
Libra Compatibility Status View Generator

Reads compatibility-matrix.yaml and produces a human-readable status report
showing progress across phases, priorities, and risk levels.

Usage:
    python3 tools/compat-status-view.py [matrix-path]
    # or from check-plan:
    # python3 tools/compat-status-view.py docs/development/compatibility-matrix.yaml
"""

import sys
import yaml
from collections import defaultdict
from datetime import datetime

def load_matrix(path):
    """Load and parse the compatibility matrix YAML."""
    try:
        with open(path, 'r', encoding='utf-8') as f:
            data = yaml.safe_load(f)
        return data.get('entries', [])
    except FileNotFoundError:
        print(f"Error: File not found: {path}", file=sys.stderr)
        sys.exit(1)
    except yaml.YAMLError as e:
        print(f"Error parsing YAML: {e}", file=sys.stderr)
        sys.exit(1)

def generate_status_view(entries):
    """Generate and print the compatibility status report."""

    print("╔" + "═" * 66 + "╗")
    print("║" + " " * 14 + "Libra Compatibility Status View - Phase 0" + " " * 11 + "║")
    print("║" + f" " * 17 + f"Generated: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}" + " " * 11 + "║")
    print("╚" + "═" * 66 + "╝")
    print()

    # Summary statistics
    total = len(entries)
    if total == 0:
        print("Error: No entries found in matrix")
        return

    done = sum(1 for e in entries if e.get('status') == 'done')
    planned = sum(1 for e in entries if e.get('status') == 'planned')
    in_progress = sum(1 for e in entries if e.get('status') == 'in-progress')
    blocked = sum(1 for e in entries if e.get('status') == 'blocked')

    evaluate = sum(1 for e in entries if e.get('status') == 'evaluate')

    print("SUMMARY")
    print(f"{'─' * 19}─ Total entries: {total}")
    print(f"{'─' * 22}─ Done: {done} ({(done/total)*100:.1f}%)")
    print(f"{'─' * 19}─ In Progress: {in_progress}")
    print(f"{'─' * 21}─ Planned: {planned}")
    print(f"{'─' * 22}─ Evaluate: {evaluate}")
    print(f"{'─' * 22}─ Blocked: {blocked}")
    print()

    # By priority
    print("BY PRIORITY")
    by_priority = defaultdict(int)
    for entry in entries:
        prio = entry.get('priority', 'unknown')
        by_priority[prio] += 1
    for prio in sorted(by_priority.keys()):
        count = by_priority[prio]
        print(f"{'─' * 19}─ {prio}: {count}")
    print()

    # By phase
    print("BY PHASE")
    by_phase = defaultdict(list)
    for entry in entries:
        phase = entry.get('phase', 'unknown')
        by_phase[phase].append(entry)

    for phase in sorted(by_phase.keys(), key=lambda x: str(x)):
        phase_entries = by_phase[phase]
        done_in_phase = sum(1 for e in phase_entries if e.get('status') == 'done')
        total_in_phase = len(phase_entries)
        pct = (done_in_phase / total_in_phase * 100) if total_in_phase > 0 else 0
        print(f"{'─' * 13}─ Phase {phase}: {done_in_phase}/{total_in_phase} done ({pct:.1f}%)")
    print()

    # Risk summary
    print("RISK DISTRIBUTION")
    by_risk = defaultdict(int)
    for entry in entries:
        risk = entry.get('risk', 'unknown')
        by_risk[risk] += 1
    for risk in sorted(by_risk.keys()):
        count = by_risk[risk]
        print(f"{'─' * 19}─ {risk}: {count}")
    print()

    # Entries with potential gaps
    print("POTENTIAL QUALITY GAPS")
    done_without_evidence = sum(
        1 for e in entries
        if e.get('status') == 'done' and not e.get('test_evidence')
    )
    done_without_cmd = sum(
        1 for e in entries
        if e.get('status') == 'done' and not e.get('verification_command')
    )
    high_risk_entries = sum(1 for e in entries if e.get('risk') == 'high')
    unclassified = sum(1 for e in entries if not e.get('action') or e.get('action') == 'unclassified')

    print(f"{'─' * 10}─ Done entries without test_evidence: {done_without_evidence}")
    print(f"{'─' * 10}─ Done entries without verification_command: {done_without_cmd}")
    print(f"{'─' * 10}─ High-risk entries: {high_risk_entries}")
    print(f"{'─' * 10}─ Unclassified entries: {unclassified}")
    print()

    # Declined references summary
    print("DECLINED REFERENCES DISTRIBUTION")
    declined_counts = defaultdict(int)
    for entry in entries:
        declined_ref = entry.get('declined_ref')
        if declined_ref:
            declined_counts[declined_ref] += 1

    if declined_counts:
        for declined_ref in sorted(declined_counts.keys()):
            count = declined_counts[declined_ref]
            print(f"{'─' * 19}─ {declined_ref}: {count} entries")
    else:
        print(f"{'─' * 25}─ (none)")
    print()

    # Status by phase breakdown
    print("DETAILED PHASE BREAKDOWN")
    for phase in sorted(by_phase.keys(), key=lambda x: str(x)):
        phase_entries = by_phase[phase]
        print(f"\n  Phase {phase}: ({len(phase_entries)} entries)")

        by_status = defaultdict(list)
        for entry in phase_entries:
            status = entry.get('status', 'unknown')
            by_status[status].append(entry)

        for status in sorted(by_status.keys()):
            status_entries = by_status[status]
            cmd_flag = f"{status_entries[0].get('command', 'unknown')}"
            if status_entries[0].get('flag'):
                cmd_flag += f" {status_entries[0].get('flag')}"

            entry_count = len(status_entries)
            print(f"    {status:15} : {entry_count:3} entries")
    print()

    # Phase 0 completion status
    print("PHASE 0 EXIT CONDITIONS")
    phase_0_entries = by_phase.get(0, [])
    if phase_0_entries:
        phase_0_done = sum(1 for e in phase_0_entries if e.get('status') == 'done')
        phase_0_total = len(phase_0_entries)
        phase_0_pct = (phase_0_done / phase_0_total * 100) if phase_0_total > 0 else 0

        # Key gate items
        pre_gates = ['PRE-1', 'PRE-2', 'PRE-3', 'PRE-4', 'PRE-5']
        pre_done = [p for p in pre_gates if any(p in str(e.get('notes', '')) for e in phase_0_entries if e.get('status') == 'done')]

        print(f"{'─' * 13}─ Phase 0 Completion: {phase_0_done}/{phase_0_total} ({phase_0_pct:.1f}%)")
        print(f"{'─' * 13}─ Key gates completed: {', '.join(pre_done) if pre_done else '(none yet)'}")
    print()

    print("For detailed matrix: docs/development/compatibility-matrix.yaml")
    print("For declined registry: docs/improvement/compatibility/declined.md")
    print("For execution plan: docs/development/compatibility.md")

def main():
    matrix_path = sys.argv[1] if len(sys.argv) > 1 else "docs/development/compatibility-matrix.yaml"

    entries = load_matrix(matrix_path)
    generate_status_view(entries)

if __name__ == '__main__':
    main()
