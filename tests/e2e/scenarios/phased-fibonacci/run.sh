#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_name="phased-fibonacci"
scenario_dir="$e2e_scenarios_root/$scenario_name"
proposal_template_file="$e2e_scenarios_root/proposals-decisions/seed_state.json"

e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"

base_ref="${ORCAS_E2E_GIT_BASE_REF:-$(git -C "$e2e_repo_root" symbolic-ref --quiet --short HEAD 2>/dev/null || echo main)}"
run_id="$E2E_RUN_ID"
artifacts_dir="$E2E_SCENARIO_ARTIFACTS_DIR"
reports_dir="$E2E_SCENARIO_REPORTS_DIR"
worktree_path="$E2E_SCENARIO_WORKTREES_DIR/lane"
branch_name="orcas/$scenario_name/$run_id"
prompt_root="$artifacts_dir/phases"
daemon_log="$E2E_SCENARIO_LOGS_DIR/orcasd.log"
state_json="$E2E_SCENARIO_XDG_DATA_HOME/orcas/state.json"

rm -rf "$E2E_SCENARIO_XDG_DIR" "$artifacts_dir" "$reports_dir" "$E2E_SCENARIO_WORKTREES_DIR"
mkdir -p "$artifacts_dir" "$reports_dir" "$prompt_root" "$(dirname "$worktree_path")"
cp "$scenario_dir/plan.md" "$artifacts_dir/plan.md"
mkdir -p "$E2E_SCENARIO_XDG_DATA_HOME/orcas" "$E2E_SCENARIO_XDG_CONFIG_HOME/orcas" "$E2E_SCENARIO_XDG_RUNTIME_HOME/orcas"
rm -f "$E2E_SCENARIO_XDG_DATA_HOME/orcas/state.db" "$E2E_SCENARIO_XDG_DATA_HOME/orcas/state.db-wal" "$E2E_SCENARIO_XDG_DATA_HOME/orcas/state.db-shm"

field_value() {
  local key="$1"
  local file="$2"
  sed -n "s/^${key}: //p" "$file" | head -n1
}

phase_label() {
  printf '%02d-%s' "$1" "$2"
}

phase_supervisor_prompt() {
  local phase="$1"
  local title="$2"
  local objective="$3"
  local instruction="$4"
  local output_file="$5"
  cat >"$output_file" <<EOF
Supervisor phase ${phase}: ${title}

Plan source:
- ${objective}

Supervisor guidance:
- Keep the work on the current phase only.
- Keep the code buildable and inspectable on disk.
- Use operator approval to gate the next phase.
- Stay out of the weeds and follow \`plan.md\`.

Worker instruction:
${instruction}
EOF
}

phase_operator_prompt() {
  local phase="$1"
  local title="$2"
  local rationale="$3"
  local output_file="$4"
  cat >"$output_file" <<EOF
Operator phase ${phase}: ${title}

Approval rationale:
${rationale}

Decision rule:
- Approve the next bounded step only if the report stays within the phase gate.
- Reject if the worker drifts beyond the current phase.
EOF
}

phase_agent_prompt() {
  local phase="$1"
  local title="$2"
  local assignment_id="$3"
  local objective="$4"
  local instruction="$5"
  local output_file="$6"
  cat >"$output_file" <<EOF
Agent phase ${phase}: ${title}

Assignment id:
- ${assignment_id}

Current objective:
- ${objective}

Worker instruction:
${instruction}

Operating rules:
- Keep the worktree buildable.
- Stay within the current phase.
- Produce code or tests on disk, not abstract notes.
EOF
}

seed_state_json() {
  local output_file="$1"
  local workstream_id="$2"
  local workstream_title="$3"
  local workstream_objective="$4"
  local workunit_id="$5"
  local workunit_title="$6"
  local task_statement="$7"
  local assignment_id="$8"
  local worker_id="$9"
  local worker_session_id="${10}"
  local report_id="${11}"
  local report_summary="${12}"
  local report_finding="${13}"
  local report_next_action="${14}"
  local report_raw_output="${15}"

  python3 - \
    "$output_file" \
    "$workstream_id" \
    "$workstream_title" \
    "$workstream_objective" \
    "$workunit_id" \
    "$workunit_title" \
    "$task_statement" \
    "$assignment_id" \
    "$worker_id" \
    "$worker_session_id" \
    "$report_id" \
    "$report_summary" \
    "$report_finding" \
    "$report_next_action" \
    "$report_raw_output" <<'PY'
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

output_path = Path(sys.argv[1])
workstream_id = sys.argv[2]
workstream_title = sys.argv[3]
workstream_objective = sys.argv[4]
workunit_id = sys.argv[5]
workunit_title = sys.argv[6]
task_statement = sys.argv[7]
assignment_id = sys.argv[8]
worker_id = sys.argv[9]
worker_session_id = sys.argv[10]
report_id = sys.argv[11]
report_summary = sys.argv[12]
report_finding = sys.argv[13]
report_next_action = sys.argv[14]
report_raw_output = sys.argv[15]

now = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
state = {
    "registry": {
        "threads": {},
        "last_connected_endpoint": None,
    },
    "thread_views": {},
    "turn_states": {},
    "collaboration": {
        "workstreams": {
            workstream_id: {
                "id": workstream_id,
                "title": workstream_title,
                "objective": workstream_objective,
                "status": "active",
                "priority": "high",
                "created_at": now,
                "updated_at": now,
            }
        },
        "authority_workstream_bridges": [],
        "work_units": {
            workunit_id: {
                "id": workunit_id,
                "workstream_id": workstream_id,
                "title": workunit_title,
                "task_statement": task_statement,
                "status": "awaiting_decision",
                "dependencies": [],
                "latest_report_id": report_id,
                "current_assignment_id": assignment_id,
                "created_at": now,
                "updated_at": now,
            }
        },
        "authority_work_unit_bridges": [],
        "assignments": {
            assignment_id: {
                "id": assignment_id,
                "work_unit_id": workunit_id,
                "plan_id": None,
                "plan_version": None,
                "plan_item_id": None,
                "execution_kind": "direct_execution",
                "alignment_rationale": None,
                "worker_id": worker_id,
                "worker_session_id": worker_session_id,
                "instructions": task_statement,
                "communication_seed": None,
                "status": "awaiting_decision",
                "attempt_number": 1,
                "created_at": now,
                "updated_at": now,
            }
        },
        "workers": {
            worker_id: {
                "id": worker_id,
                "kind": "harness",
                "status": "idle",
                "current_assignment_id": assignment_id,
            }
        },
        "worker_sessions": {
            worker_session_id: {
                "id": worker_session_id,
                "worker_id": worker_id,
                "backend_type": "codex_thread",
                "thread_id": None,
                "tracked_thread_id": None,
                "active_turn_id": None,
                "runtime_status": "idle",
                "attachability": "not_attachable",
                "updated_at": now,
            }
        },
        "reports": {
            report_id: {
                "id": report_id,
                "work_unit_id": workunit_id,
                "assignment_id": assignment_id,
                "worker_id": worker_id,
                "disposition": "completed",
                "summary": report_summary,
                "findings": [report_finding],
                "blockers": [],
                "questions": [],
                "recommended_next_actions": [report_next_action],
                "confidence": "high",
                "raw_output": report_raw_output,
                "parse_result": "parsed",
                "needs_supervisor_review": False,
                "created_at": now,
            }
        },
        "decisions": {},
        "assignment_communications": {},
        "workspace_operations": {},
        "landing_authorizations": {},
        "landing_executions": {},
        "supervisor_proposals": {},
        "codex_thread_assignments": {},
        "supervisor_turn_decisions": {},
        "planning": {},
    },
}

output_path.write_text(json.dumps(state, indent=2) + "\n")
PY
}

write_phase_1_skeleton() {
  cat >"$worktree_path/main.c" <<'EOF'
#include <stdio.h>

static void print_fibonacci(int count) {
    int a = 0;
    int b = 1;

    for (int i = 0; i < count; ++i) {
        if (i > 0) {
            putchar(' ');
        }
        printf("%d", a);
        int next = a + b;
        a = b;
        b = next;
    }
    putchar('\n');
}

int main(void) {
    print_fibonacci(7);
    return 0;
}
EOF

  cat >"$worktree_path/Makefile" <<'EOF'
CC ?= cc
CFLAGS ?= -O2 -Wall -Wextra -pedantic
TARGET := fibonacci

.PHONY: test clean

test: $(TARGET)
	./$(TARGET)

$(TARGET): main.c
	$(CC) $(CFLAGS) main.c -o $(TARGET)

clean:
	rm -f $(TARGET)
EOF
}

write_phase_2_cli() {
  cat >"$worktree_path/main.c" <<'EOF'
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int parse_positive_int(const char *text, int *value) {
    char *end = NULL;
    long parsed;

    errno = 0;
    parsed = strtol(text, &end, 10);
    if (errno != 0 || end == text || *end != '\0' || parsed <= 0 || parsed > INT_MAX) {
        return 0;
    }

    *value = (int)parsed;
    return 1;
}

static void print_fibonacci(int count, const char *separator) {
    int a = 0;
    int b = 1;

    for (int i = 0; i < count; ++i) {
        if (i > 0) {
            fputs(separator, stdout);
        }
        printf("%d", a);
        int next = a + b;
        a = b;
        b = next;
    }
    putchar('\n');
}

static void usage(const char *program) {
    fprintf(stderr, "usage: %s [--count N] [--separator TEXT]\n", program);
}

int main(int argc, char **argv) {
    int count = 7;
    const char *separator = " ";

    for (int i = 1; i < argc; ++i) {
        if (strcmp(argv[i], "--count") == 0) {
            if (i + 1 >= argc || !parse_positive_int(argv[++i], &count)) {
                usage(argv[0]);
                return 1;
            }
        } else if (strcmp(argv[i], "--separator") == 0) {
            if (i + 1 >= argc) {
                usage(argv[0]);
                return 1;
            }
            separator = argv[++i];
        } else if (strcmp(argv[i], "--help") == 0) {
            usage(argv[0]);
            return 0;
        } else {
            usage(argv[0]);
            return 1;
        }
    }

    print_fibonacci(count, separator);
    return 0;
}
EOF

  cat >"$worktree_path/Makefile" <<'EOF'
CC ?= cc
CFLAGS ?= -O2 -Wall -Wextra -pedantic
TARGET := fibonacci

.PHONY: test clean

test: $(TARGET)
	./$(TARGET) --count 7

$(TARGET): main.c
	$(CC) $(CFLAGS) main.c -o $(TARGET)

clean:
	rm -f $(TARGET)
EOF
}

write_phase_3_library_split() {
  cat >"$worktree_path/fib.h" <<'EOF'
#ifndef FIB_H
#define FIB_H

#include <stdio.h>

void fib_print(FILE *out, int count, const char *separator);

#endif
EOF

  cat >"$worktree_path/fib.c" <<'EOF'
#include "fib.h"

void fib_print(FILE *out, int count, const char *separator) {
    int a = 0;
    int b = 1;

    for (int i = 0; i < count; ++i) {
        if (i > 0) {
            fputs(separator, out);
        }
        fprintf(out, "%d", a);
        int next = a + b;
        a = b;
        b = next;
    }
    fputc('\n', out);
}
EOF

  cat >"$worktree_path/main.c" <<'EOF'
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "fib.h"

static int parse_positive_int(const char *text, int *value) {
    char *end = NULL;
    long parsed;

    errno = 0;
    parsed = strtol(text, &end, 10);
    if (errno != 0 || end == text || *end != '\0' || parsed <= 0 || parsed > INT_MAX) {
        return 0;
    }

    *value = (int)parsed;
    return 1;
}

static void usage(const char *program) {
    fprintf(stderr, "usage: %s [--count N] [--separator TEXT]\n", program);
}

int main(int argc, char **argv) {
    int count = 7;
    const char *separator = " ";

    for (int i = 1; i < argc; ++i) {
        if (strcmp(argv[i], "--count") == 0) {
            if (i + 1 >= argc || !parse_positive_int(argv[++i], &count)) {
                usage(argv[0]);
                return 1;
            }
        } else if (strcmp(argv[i], "--separator") == 0) {
            if (i + 1 >= argc) {
                usage(argv[0]);
                return 1;
            }
            separator = argv[++i];
        } else if (strcmp(argv[i], "--help") == 0) {
            usage(argv[0]);
            return 0;
        } else {
            usage(argv[0]);
            return 1;
        }
    }

    fib_print(stdout, count, separator);
    return 0;
}
EOF

  cat >"$worktree_path/Makefile" <<'EOF'
CC ?= cc
CFLAGS ?= -O2 -Wall -Wextra -pedantic
TARGET := fibonacci

.PHONY: test clean

test: $(TARGET)
	./$(TARGET) --count 7

$(TARGET): main.c fib.c fib.h
	$(CC) $(CFLAGS) main.c fib.c -o $(TARGET)

clean:
	rm -f $(TARGET)
EOF
}

write_phase_4_tests() {
  mkdir -p "$worktree_path/tests"

  cat >"$worktree_path/tests/test_fibonacci.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

expected_default='0 1 1 2 3 5 8'
expected_custom='0,1,1,2,3'

actual_default="$(./fibonacci --count 7)"
actual_custom="$(./fibonacci --count 5 --separator ,)"

test "$actual_default" = "$expected_default"
test "$actual_custom" = "$expected_custom"

printf 'PASS\n'
EOF
  chmod +x "$worktree_path/tests/test_fibonacci.sh"

  cat >"$worktree_path/Makefile" <<'EOF'
CC ?= cc
CFLAGS ?= -O2 -Wall -Wextra -pedantic
TARGET := fibonacci

.PHONY: test clean

test: $(TARGET)
	./tests/test_fibonacci.sh

$(TARGET): main.c fib.c fib.h
	$(CC) $(CFLAGS) main.c fib.c -o $(TARGET)

clean:
	rm -f $(TARGET)
EOF
}

daemon_cli_pid=""

start_daemon() {
  ./bin/orcas.sh daemon start --force-spawn >"$daemon_log" 2>&1 &
  daemon_cli_pid=$!
  sleep 4
}

stop_daemon() {
  ./bin/orcas.sh daemon stop >/dev/null 2>&1 || true
  if [[ -n "$daemon_cli_pid" ]]; then
    wait "$daemon_cli_pid" >/dev/null 2>&1 || true
    daemon_cli_pid=""
  fi
}

reset_authority_store() {
  rm -f "$ORCAS_E2E_XDG_DATA_HOME/orcas/state.db" "$ORCAS_E2E_XDG_DATA_HOME/orcas/state.db-wal" "$ORCAS_E2E_XDG_DATA_HOME/orcas/state.db-shm"
}

seed_report_state() {
  local workunit_id="$1"
  local assignment_id="$2"
  local report_id="$3"
  local summary="$4"
  local finding="$5"
  local next_action="$6"
  local raw_output="$7"

  python3 - "$state_json" "$workunit_id" "$assignment_id" "$report_id" "$summary" "$finding" "$next_action" "$raw_output" <<'PY'
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

state_path = Path(sys.argv[1])
workunit_id = sys.argv[2]
assignment_id = sys.argv[3]
report_id = sys.argv[4]
summary = sys.argv[5]
finding = sys.argv[6]
next_action = sys.argv[7]
raw_output = sys.argv[8]

obj = json.loads(state_path.read_text())
coll = obj["collaboration"]
assignment = coll["assignments"][assignment_id]
worker_id = assignment["worker_id"]
now = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")

coll["reports"][report_id] = {
    "id": report_id,
    "work_unit_id": workunit_id,
    "assignment_id": assignment_id,
    "worker_id": worker_id,
    "disposition": "completed",
    "summary": summary,
    "findings": [finding],
    "blockers": [],
    "questions": [],
    "recommended_next_actions": [next_action],
    "confidence": "high",
    "raw_output": raw_output,
    "parse_result": "parsed",
    "needs_supervisor_review": False,
    "created_at": now,
}

work_unit = coll["work_units"][workunit_id]
work_unit["latest_report_id"] = report_id
work_unit["current_assignment_id"] = assignment_id
work_unit["status"] = "awaiting_decision"
work_unit["updated_at"] = now

assignment["status"] = "awaiting_decision"
assignment["updated_at"] = now

if worker_id in coll["workers"]:
    coll["workers"][worker_id]["current_assignment_id"] = assignment_id

state_path.write_text(json.dumps(obj, indent=2) + "\n")
PY
}

seed_open_proposal() {
  local proposal_id="$1"
  local workstream_id="$2"
  local workstream_title="$3"
  local workstream_objective="$4"
  local workunit_id="$5"
  local workunit_title="$6"
  local task_statement="$7"
  local assignment_id="$8"
  local worker_id="$9"
  local worker_session_id="${10}"
  local report_id="${11}"
  local phase_label="${12}"
  local decision_type="${13}"
  local rationale="${14}"
  local next_objective="${15}"
  local next_instruction="${16}"

  python3 - \
    "$state_json" \
    "$proposal_template_file" \
    "$proposal_id" \
    "$workstream_id" \
    "$workstream_title" \
    "$workstream_objective" \
    "$workunit_id" \
    "$workunit_title" \
    "$task_statement" \
    "$assignment_id" \
    "$worker_id" \
    "$worker_session_id" \
    "$report_id" \
    "$phase_label" \
    "$decision_type" \
    "$rationale" \
    "$next_objective" \
    "$next_instruction" <<'PY'
import copy
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

state_path = Path(sys.argv[1])
template_path = Path(sys.argv[2])
proposal_id = sys.argv[3]
workstream_id = sys.argv[4]
workstream_title = sys.argv[5]
workstream_objective = sys.argv[6]
workunit_id = sys.argv[7]
workunit_title = sys.argv[8]
task_statement = sys.argv[9]
assignment_id = sys.argv[10]
worker_id = sys.argv[11]
worker_session_id = sys.argv[12]
report_id = sys.argv[13]
phase_label = sys.argv[14]
decision_type = sys.argv[15]
rationale = sys.argv[16]
next_objective = sys.argv[17]
next_instruction = sys.argv[18]

now = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
state = json.loads(state_path.read_text())
template = json.loads(template_path.read_text())
proposal = copy.deepcopy(next(iter(template["collaboration"]["supervisor_proposals"].values())))
work_unit = state["collaboration"]["work_units"][workunit_id]
report = state["collaboration"]["reports"][report_id]
decisions = [
    decision
    for decision in state["collaboration"].get("decisions", {}).values()
    if decision.get("work_unit_id") == workunit_id
]
decisions.sort(key=lambda item: (item.get("created_at", ""), item.get("id", "")))
latest_decision = decisions[-1] if decisions else None

proposal["id"] = proposal_id
proposal["workstream_id"] = workstream_id
proposal["primary_work_unit_id"] = workunit_id
proposal["source_report_id"] = report_id
proposal["status"] = "open"
proposal["created_at"] = now
proposal["reasoner_backend"] = "responses_api"
proposal["reasoner_model"] = "gpt-5.4"
proposal["reasoner_response_id"] = None
proposal["reasoner_usage"] = None
proposal["reasoner_output_text"] = None
proposal["validated_at"] = now
proposal["reviewed_at"] = None
proposal["reviewed_by"] = None
proposal["review_note"] = None
proposal["approved_decision_id"] = None
proposal["approved_assignment_id"] = None
proposal["approval_edits"] = None
proposal["approved_proposal"] = None
proposal["generation_failure"] = None

proposal["trigger"] = {
    "kind": "human_requested",
    "requested_at": now,
    "requested_by": "harness",
    "source_report_id": report_id,
    "note": f"Seeded open proposal for phase {phase_label}.",
}

proposal["summary"] = {
    "headline": f"Phase {phase_label} proposal",
    "situation": f"The phased Fibonacci work is ready for {phase_label}.",
    "recommended_action": rationale,
    "key_evidence": [
        "The work unit has a seeded report and a known current assignment.",
        "The next bounded step stays within the current phase gate.",
    ],
    "risks": [],
    "review_focus": [
        "Keep the next step bounded.",
        "Do not drift beyond the current phase.",
    ],
}

requires_assignment = decision_type in {"continue", "redirect"}
proposal["proposal"] = {
    "schema_version": "supervisor_proposal.v2",
    "summary": {
        "headline": f"Phase {phase_label} proposal",
        "situation": f"Supervisor planning for phase {phase_label}.",
        "recommended_action": rationale,
        "key_evidence": [
            "The current report is seeded and reviewable.",
            "The next phase has a concrete bounded scope.",
        ],
        "risks": [],
        "review_focus": [
            "Confirm the next step stays on track.",
        ],
    },
    "proposed_decision": {
        "decision_type": decision_type,
        "target_work_unit_id": workunit_id,
        "source_report_id": report_id,
        "rationale": rationale,
        "expected_work_unit_status": "ready" if requires_assignment else "completed",
        "requires_assignment": requires_assignment,
    },
    "draft_next_assignment": {
        "target_work_unit_id": workunit_id,
        "predecessor_assignment_id": assignment_id,
        "derived_from_decision_type": decision_type,
        "plan_id": None,
        "plan_version": None,
        "plan_item_id": None,
        "execution_kind": "direct_execution",
        "alignment_rationale": None,
        "preferred_worker_id": worker_id,
        "worker_kind": "codex",
        "objective": next_objective if requires_assignment else "",
        "instructions": [next_instruction] if requires_assignment else [],
        "acceptance_criteria": [
            "The phase stays bounded and the code remains buildable.",
        ]
        if requires_assignment
        else [],
        "stop_conditions": [
            "Stop if the phase gate changes.",
        ]
        if requires_assignment
        else [],
        "required_context_refs": [report_id] if requires_assignment else [],
        "expected_report_fields": [
            "summary",
            "findings",
            "questions",
        ]
        if requires_assignment
        else [],
        "boundedness_note": f"Stay within phase {phase_label}.",
    }
    if requires_assignment
    else None,
    "confidence": "high",
    "plan_assessment": None,
    "plan_revision_proposal": None,
    "warnings": [],
    "open_questions": [],
}

context_pack = proposal["context_pack"]
context_pack["generated_at"] = now
context_pack["trigger"] = proposal["trigger"]
context_pack["state_anchor"]["workstream_id"] = workstream_id
context_pack["state_anchor"]["primary_work_unit_id"] = workunit_id
context_pack["state_anchor"]["source_report_id"] = report_id
context_pack["state_anchor"]["current_assignment_id"] = assignment_id
context_pack["state_anchor"]["primary_work_unit_updated_at"] = work_unit["updated_at"]
context_pack["state_anchor"]["source_report_created_at"] = report["created_at"]
context_pack["state_anchor"]["latest_decision_id"] = (
    latest_decision["id"] if latest_decision is not None else None
)
context_pack["state_anchor"]["latest_decision_created_at"] = (
    latest_decision["created_at"] if latest_decision is not None else None
)
context_pack["workstream"]["id"] = workstream_id
context_pack["workstream"]["title"] = workstream_title
context_pack["workstream"]["objective"] = workstream_objective
context_pack["workstream"]["status"] = "active"
context_pack["primary_work_unit"]["id"] = workunit_id
context_pack["primary_work_unit"]["title"] = workunit_title
context_pack["primary_work_unit"]["task_statement"] = task_statement
context_pack["primary_work_unit"]["status"] = "awaiting_decision"
context_pack["primary_work_unit"]["current_assignment_id"] = assignment_id
context_pack["primary_work_unit"]["latest_report_id"] = report_id
context_pack["source_report"]["id"] = report_id
context_pack["source_report"]["assignment_id"] = assignment_id
context_pack["source_report"]["worker_id"] = worker_id
context_pack["source_report"]["worker_session_id"] = worker_session_id
context_pack["source_report"]["summary"] = f"Phase {phase_label} report seeded by the harness."
context_pack["source_report"]["findings"] = [
    f"Phase {phase_label} is ready for operator approval."
]
context_pack["source_report"]["recommended_next_actions"] = [next_instruction or rationale]
context_pack["source_report"]["raw_output_excerpt"] = "seeded raw output"
context_pack["source_report"]["submitted_at"] = report["created_at"]
context_pack["current_assignment"]["id"] = assignment_id
context_pack["current_assignment"]["status"] = "awaiting_decision"
context_pack["current_assignment"]["worker_id"] = worker_id
context_pack["current_assignment"]["worker_session_id"] = worker_session_id
context_pack["current_assignment"]["instructions"] = task_statement
context_pack["current_assignment"]["created_at"] = now
context_pack["current_assignment"]["updated_at"] = now
context_pack["worker_session"]["id"] = worker_session_id
context_pack["worker_session"]["worker_id"] = worker_id
context_pack["worker_session"]["runtime_status"] = "idle"
context_pack["worker_session"]["attachability"] = "not_attachable"
context_pack["worker_session"]["updated_at"] = now

state["collaboration"]["supervisor_proposals"][proposal_id] = proposal
state_path.write_text(json.dumps(state, indent=2) + "\n")
PY
}

phase_titles=(
  "scope"
  "skeleton"
  "cli-and-validation"
  "library-split"
  "tests-and-polish"
)
phase_objectives=(
  "Read plan.md and summarize risks, assumptions, and the implementation order."
  "Create a buildable C skeleton with main.c and a Makefile."
  "Add CLI arguments for Fibonacci length and formatting with clean validation."
  "Move Fibonacci logic into fib.c and fib.h while preserving behavior."
  "Add repeatable tests, clean warnings, and finish the project."
)
phase_instructions=(
  "Read plan.md and produce a concise scoping report. Do not edit files."
  "Implement phase 1 only. Create main.c and a Makefile, keep a smoke-test target, and make the project buildable."
  "Implement phase 2 only. Add CLI arguments for sequence length and formatting, and fail cleanly on invalid input."
  "Implement phase 3 only. Split the Fibonacci logic into fib.c and fib.h while keeping the CLI thin."
  "Implement phase 4 only. Add a repeatable test script, tighten warnings, and finish the project."
)
phase_rationales=(
  "The plan is scoped and the phase gate is ready."
  "The skeleton is buildable and the next bounded step is clear."
  "The CLI and validation are in phase and should continue."
  "The library split is in phase and should continue."
  "The implementation is complete and should be marked done."
)
phase_decisions=(continue continue continue continue mark_complete)

rm -rf "$worktree_path"
git worktree prune --expire now >/dev/null 2>&1 || true
git worktree add -f -b "$branch_name" "$worktree_path" "$base_ref" >"$reports_dir/git-worktree-add.txt"

workstream_id="ws-phased-fibonacci"
workunit_id="wu-phased-fibonacci"
worker_id="worker-phased-fibonacci"
worker_session_id="session-phased-fibonacci"
phase0_assignment_id="assignment-phased-0"
phase0_report_id="report-phased-0"
phase0_proposal_id="proposal-phased-0"

seed_state_json \
  "$state_json" \
  "$workstream_id" \
  "Phased Fibonacci" \
  "Validate a multi-phase supervisor/operator workflow on a real C codebase" \
  "$workunit_id" \
  "Phased Fibonacci C implementation" \
  "Build a Fibonacci CLI in phases and keep the worktree buildable after every approval." \
  "$phase0_assignment_id" \
  "$worker_id" \
  "$worker_session_id" \
  "$phase0_report_id" \
  "Scoped the phased Fibonacci workflow and agreed on the implementation order." \
  "The plan is visible, the worktree lane exists, and the next step is ready." \
  "Proceed to the skeleton phase." \
  "scoping report seeded by the harness"
seed_open_proposal \
  "$phase0_proposal_id" \
  "$workstream_id" \
  "Phased Fibonacci" \
  "Validate a multi-phase supervisor/operator workflow on a real C codebase" \
  "$workunit_id" \
  "Phased Fibonacci C implementation" \
  "Build a Fibonacci CLI in phases and keep the worktree buildable after every approval." \
  "$phase0_assignment_id" \
  "$worker_id" \
  "$worker_session_id" \
  "$phase0_report_id" \
  "phase 0" \
  "${phase_decisions[0]}" \
  "${phase_rationales[0]}" \
  "${phase_objectives[1]}" \
  "${phase_instructions[1]}"

reset_authority_store
start_daemon

current_report_id="$phase0_report_id"
current_assignment_id="$phase0_assignment_id"
current_proposal_id="$phase0_proposal_id"

for phase in 0 1 2 3 4; do
  phase_name="${phase_titles[$phase]}"
  phase_dir="$prompt_root/$(phase_label "$phase" "$phase_name")"
  report_dir="$reports_dir/$(phase_label "$phase" "$phase_name")"
  mkdir -p "$phase_dir" "$report_dir"

  supervisor_file="$phase_dir/supervisor-prompt.txt"
  operator_file="$phase_dir/operator-prompt.txt"
  agent_file="$phase_dir/agent-prompt.txt"
  proposal_md="$phase_dir/proposal.md"
  proposal_get_stdout="$report_dir/proposal-get.txt"
  approval_stdout="$report_dir/proposal-approve.txt"
  workunit_stdout="$report_dir/workunit-get.txt"

  phase_supervisor_prompt \
    "$phase" \
    "$phase_name" \
    "${phase_objectives[$phase]}" \
    "${phase_instructions[$phase]}" \
    "$supervisor_file"
  phase_operator_prompt \
    "$phase" \
    "$phase_name" \
    "${phase_rationales[$phase]}" \
    "$operator_file"

  phase_agent_prompt \
    "$phase" \
    "$phase_name" \
    "$current_assignment_id" \
    "${phase_objectives[$phase]}" \
    "${phase_instructions[$phase]}" \
    "$agent_file"

  ./bin/orcas.sh proposals get --proposal "$current_proposal_id" >"$proposal_get_stdout"
  cat >"$proposal_md" <<EOF
# Phase ${phase}: ${phase_name}

- Proposal id: ${current_proposal_id}
- Work unit id: ${workunit_id}
- Source report id: ${current_report_id}
- Supervisor rationale: ${phase_rationales[$phase]}
- Current objective: ${phase_objectives[$phase]}

This proposal was seeded by the harness so the approval flow stays deterministic.
EOF

  approval_args=(
    ./bin/orcas.sh proposals approve
    --proposal "$current_proposal_id"
    --reviewed-by operator
    --review-note "${phase_rationales[$phase]}"
    --rationale "${phase_rationales[$phase]}"
  )
  if [[ "${phase_decisions[$phase]}" == "continue" ]]; then
    approval_args+=(
      --type continue
      --objective "${phase_objectives[$((phase + 1))]}"
      --instruction "${phase_instructions[$((phase + 1))]}"
      --acceptance "${phase_rationales[$phase]}"
      --stop-condition "stay within phase ${phase}"
      --expected-report-field "summary"
      --expected-report-field "findings"
      --expected-report-field "questions"
    )
  else
    approval_args+=(--type mark-complete)
  fi

  approval_output="$("${approval_args[@]}")"
  printf '%s\n' "$approval_output" >"$approval_stdout"
  approved_decision_id="$(field_value approved_decision_id "$approval_stdout")"
  approved_assignment_id="$(field_value approved_assignment_id "$approval_stdout")"
  test -n "$approved_decision_id"
  if [[ "${phase_decisions[$phase]}" == "continue" ]]; then
    test -n "$approved_assignment_id"
  fi

  ./bin/orcas.sh workunits get --workunit "$workunit_id" >"$workunit_stdout"

  if [[ "${phase_decisions[$phase]}" == "continue" ]]; then
    next_assignment_dir="$prompt_root/$(phase_label "$((phase + 1))" "${phase_titles[$((phase + 1))]}")"
    mkdir -p "$next_assignment_dir"
    phase_agent_prompt \
      "$((phase + 1))" \
      "${phase_titles[$((phase + 1))]}" \
      "$approved_assignment_id" \
      "${phase_objectives[$((phase + 1))]}" \
      "${phase_instructions[$((phase + 1))]}" \
      "$next_assignment_dir/agent-prompt.txt"
    current_assignment_id="$approved_assignment_id"
  fi

  if [[ "$phase" -eq 0 ]]; then
    write_phase_1_skeleton
  elif [[ "$phase" -eq 1 ]]; then
    write_phase_2_cli
  elif [[ "$phase" -eq 2 ]]; then
    write_phase_3_library_split
  elif [[ "$phase" -eq 3 ]]; then
    write_phase_4_tests
  fi

  if [[ "${phase_decisions[$phase]}" == "continue" ]]; then
    next_report_id="report-phased-$((phase + 1))"
    next_proposal_id="proposal-phased-$((phase + 1))"
    next_summary="Phase $((phase + 1)) completed on disk by the harness."
    next_finding="The phase $((phase + 1)) code path is present in the worktree."
    next_action="${phase_instructions[$((phase + 1))]}"
    stop_daemon
    seed_report_state \
      "$workunit_id" \
      "$approved_assignment_id" \
      "$next_report_id" \
      "$next_summary" \
      "$next_finding" \
      "$next_action" \
      "phase $((phase + 1)) seeded by the harness"
    seed_open_proposal \
      "$next_proposal_id" \
      "$workstream_id" \
      "Phased Fibonacci" \
      "Validate a multi-phase supervisor/operator workflow on a real C codebase" \
      "$workunit_id" \
      "Phased Fibonacci C implementation" \
      "Build a Fibonacci CLI in phases and keep the worktree buildable after every approval." \
      "$approved_assignment_id" \
      "$worker_id" \
      "$worker_session_id" \
      "$next_report_id" \
      "phase $((phase + 1))" \
      "${phase_decisions[$((phase + 1))]}" \
      "${phase_rationales[$phase]}" \
      "${phase_objectives[$((phase + 1))]}" \
      "${phase_instructions[$((phase + 1))]}"
    reset_authority_store
    start_daemon
    current_report_id="$next_report_id"
    current_proposal_id="$next_proposal_id"
  fi
done

make -C "$worktree_path" test >"$reports_dir/final-make-test.txt"
git -C "$worktree_path" status --short >"$reports_dir/final-git-status.txt"

stop_daemon

echo "PASS"
