import { Type } from "typebox";
import { SpoolClient, configFromEnv } from "./client";
import type { ExtensionAPI, SpoolToolResult } from "./types";

const Identifier = Type.String({ description: "Task Identifier, such as SPOOL-1" });
const CriterionStatus = Type.Union([Type.Literal("pending"), Type.Literal("satisfied"), Type.Literal("waived")]);
const ValidationStatus = Type.Union([
  Type.Literal("pending"),
  Type.Literal("passed"),
  Type.Literal("failed"),
  Type.Literal("waived"),
]);
const Priority = Type.Union([Type.Literal("urgent"), Type.Literal("high"), Type.Literal("normal"), Type.Literal("low")]);
const ChildTaskState = Type.Union([Type.Literal("backlog"), Type.Literal("ready")]);
const DelegationTaskState = Type.Union([Type.Literal("backlog"), Type.Literal("ready")]);
const TaskState = Type.Union([
  Type.Literal("backlog"),
  Type.Literal("ready"),
  Type.Literal("in_progress"),
  Type.Literal("human_review"),
  Type.Literal("rework"),
  Type.Literal("integrating"),
  Type.Literal("done"),
  Type.Literal("canceled"),
]);
const WorkpadParams = Type.Object({ identifier: Identifier, body: Type.String() });
const AppendWorkpadParams = Type.Object({
  identifier: Identifier,
  body: Type.String({ description: "Markdown to append to the current Workpad Note." }),
  separator: Type.Optional(Type.String({ description: "Separator inserted before appended text; defaults to a blank line." })),
});
const AcceptanceCriterionStatusParams = Type.Object({
  identifier: Identifier,
  position: Type.Number({ description: "1-based requirement position" }),
  status: CriterionStatus,
  waiver_reason: Type.Optional(Type.String()),
});
const ValidationItemStatusParams = Type.Object({
  identifier: Identifier,
  position: Type.Number({ description: "1-based requirement position" }),
  status: ValidationStatus,
  waiver_reason: Type.Optional(Type.String()),
  validated_base_commit: Type.Optional(Type.String({ description: "Main Branch commit that the validation evidence was run against." })),
});
const ChildTaskParams = Type.Object({
  parent_identifier: Identifier,
  title: Type.String(),
  brief: Type.String(),
  acceptance_criteria: Type.Array(Type.String()),
  validation_items: Type.Array(Type.String()),
  priority: Type.Optional(Priority),
  state: Type.Optional(ChildTaskState),
  tags: Type.Optional(Type.Array(Type.String())),
  review_required: Type.Optional(Type.Boolean()),
  blocks_parent: Type.Optional(Type.Boolean()),
});
const TaskLinkParams = Type.Object({
  identifier: Identifier,
  kind: Type.String({ description: "Task Link kind, such as local_worktree, task_branch, review_artifact, or external_reference." }),
  target: Type.String({ description: "Task Link target such as a path, branch name, URL, or artifact identifier." }),
  label: Type.Optional(Type.String({ description: "Optional human-readable Task Link label." })),
  is_primary: Type.Optional(Type.Boolean({ description: "Whether this Task Link should become the Primary Handoff Link for the Task." })),
});
const TransitionParams = Type.Object({
  identifier: Identifier,
  to_state: TaskState,
  agent_run_id: Type.Optional(Type.String()),
});
const ReviewDecisionParams = Type.Object({
  identifier: Identifier,
  decision: Type.Union([Type.Literal("approve"), Type.Literal("rework")]),
  feedback: Type.Optional(Type.String({ description: "Concise human feedback; required by Spool for rework decisions." })),
});
const DelegationTaskDraftParams = Type.Object({
  queue_key: Type.String({ description: "Task Queue Key for the new Root Task." }),
  title: Type.String(),
  brief: Type.String({ description: "Task Brief Markdown narrative." }),
  priority: Type.Optional(Priority),
  initial_state: Type.Optional(DelegationTaskState),
  review_required: Type.Optional(Type.Boolean()),
  tags: Type.Optional(Type.Array(Type.String())),
  conflict_hints: Type.Optional(Type.Array(Type.String({ description: "Advisory Task Conflict Hint target." }))),
  blocking_task_identifiers: Type.Optional(Type.Array(Identifier)),
  acceptance_criteria: Type.Optional(Type.Array(Type.String())),
  validation_items: Type.Optional(Type.Array(Type.String())),
});
const RefineBacklogTaskParams = Type.Object({
  identifier: Identifier,
  title: Type.Optional(Type.String()),
  brief: Type.Optional(Type.String({ description: "Task Brief Markdown narrative." })),
  priority: Type.Optional(Priority),
  target_state: Type.Optional(DelegationTaskState),
  review_required: Type.Optional(Type.Boolean()),
  tags: Type.Optional(Type.Array(Type.String())),
  conflict_hints: Type.Optional(Type.Array(Type.String({ description: "Replacement advisory Task Conflict Hint targets." }))),
  blocking_task_identifiers: Type.Optional(Type.Array(Identifier)),
  acceptance_criteria: Type.Optional(Type.Array(Type.String({ description: "Replacement ordered Acceptance Criteria." }))),
  validation_items: Type.Optional(Type.Array(Type.String({ description: "Replacement ordered Validation Items." }))),
});
const WorkerStatus = Type.Union([
  Type.Literal("completion_intent"),
  Type.Literal("blocked"),
  Type.Literal("retryable_failure"),
]);
const WorkerStatusParams = Type.Object({
  identifier: Identifier,
  status: WorkerStatus,
  message: Type.Optional(Type.String()),
  agent_run_id: Type.Optional(Type.String()),
});

function asToolResult(details: unknown): SpoolToolResult {
  return {
    content: [{ type: "text", text: JSON.stringify(details, null, 2) }],
    details,
  };
}

export default function registerSpoolExtension(pi: ExtensionAPI) {
  const config = configFromEnv();
  const client = new SpoolClient(config);

  pi.registerTool({
    name: "spool_get_task",
    label: "Spool: Get Task",
    description: "Fetch full Spool Task context by Task Identifier.",
    parameters: Type.Object({ identifier: Identifier }),
    async execute(_id, params, signal) {
      return asToolResult(await client.getTask(params.identifier, signal));
    },
  });

  pi.registerTool({
    name: "spool_get_task_context_bundle",
    label: "Spool: Get Task Context Bundle",
    description:
      "Fetch the read-only Spool-owned run-start context bundle, including advisory Task Conflict Hints and likely files/path guidance, for a Worker Agent without raw transcripts, raw launcher payloads, secrets, or unrelated queue data.",
    parameters: Type.Object({ identifier: Identifier }),
    async execute(_id, params, signal) {
      return asToolResult(await client.getTaskContextBundle(params.identifier, signal));
    },
  });

  pi.registerTool({
    name: "spool_update_workpad",
    label: "Spool: Update Workpad",
    description: "Replace the Task's singleton Workpad Note body.",
    parameters: WorkpadParams,
    async execute(_id, params, signal) {
      return asToolResult(await client.updateWorkpad(params.identifier, config.actor, params.body, signal));
    },
  });

  pi.registerTool({
    name: "spool_append_workpad",
    label: "Spool: Append Workpad",
    description: "Append Markdown to the current Workpad Note without manually replacing the whole note.",
    parameters: AppendWorkpadParams,
    async execute(_id, params, signal) {
      return asToolResult(
        await client.appendWorkpad(params.identifier, config.actor, params.body, params.separator ?? "\n\n", signal),
      );
    },
  });

  pi.registerTool({
    name: "spool_set_acceptance_criterion_status",
    label: "Spool: Set Acceptance Criterion Status",
    description: "Set an Acceptance Criterion status by 1-based position.",
    parameters: AcceptanceCriterionStatusParams,
    async execute(_id, params, signal) {
      return asToolResult(await client.setAcceptanceCriterionStatus(params, config.actor, signal));
    },
  });

  pi.registerTool({
    name: "spool_set_validation_item_status",
    label: "Spool: Set Validation Item Status",
    description: "Set a Validation Item status by 1-based position.",
    parameters: ValidationItemStatusParams,
    async execute(_id, params, signal) {
      return asToolResult(await client.setValidationItemStatus(params, config.actor, signal));
    },
  });

  pi.registerTool({
    name: "spool_create_child_task",
    label: "Spool: Create Child Task",
    description: "Create a same-queue Child Task from the current Task context.",
    parameters: ChildTaskParams,
    async execute(_id, params, signal) {
      return asToolResult(await client.createChildTask(params, config.actor, signal));
    },
  });

  pi.registerTool({
    name: "spool_attach_task_link",
    label: "Spool: Attach Task Link",
    description:
      "Attach or upsert a typed Task Link collaboration/delivery reference on a Task. Task Links are not authoritative Acceptance Criteria, Validation Items, or scheduling gates.",
    parameters: TaskLinkParams,
    async execute(_id, params, signal) {
      return asToolResult(
        await client.upsertTaskLink(
          params.identifier,
          {
            kind: params.kind,
            target: params.target,
            label: params.label,
            is_primary: params.is_primary,
          },
          config.actor,
          signal,
        ),
      );
    },
  });

  pi.registerTool({
    name: "spool_request_transition",
    label: "Spool: Request State Transition",
    description: "Request a normal Task State Transition through Spool gates.",
    parameters: TransitionParams,
    async execute(_id, params, signal) {
      return asToolResult(
        await client.requestTransition(
          params.identifier,
          params.to_state,
          config.actor,
          params.agent_run_id ?? config.agentRunId,
          signal,
        ),
      );
    },
  });

  pi.registerTool({
    name: "spool_record_review_decision",
    label: "Spool: Record Review Decision",
    description: "Record a human approve or rework Review Decision for a Task in Human Review through the deterministic Spool API path.",
    parameters: ReviewDecisionParams,
    async execute(_id, params, signal) {
      return asToolResult(await client.recordReviewDecision(params, config.actor, signal));
    },
  });

  pi.registerTool({
    name: "spool_create_delegated_root_task",
    label: "Spool: Create Delegated Root Task",
    description:
      "Create one Root Task from a Delegation Session through the deterministic Delegation Task draft helper. Use only in human-present Interactive Agent Sessions.",
    parameters: DelegationTaskDraftParams,
    async execute(_id, params, signal) {
      return asToolResult(await client.createDelegatedRootTask(params, config.actor, signal));
    },
  });

  pi.registerTool({
    name: "spool_refine_backlog_task",
    label: "Spool: Refine Backlog Task",
    description:
      "Refine an existing Backlog Task from a Delegation Session through the deterministic Backlog Task refinement helper. Use only in human-present Interactive Agent Sessions.",
    parameters: RefineBacklogTaskParams,
    async execute(_id, params, signal) {
      return asToolResult(await client.refineBacklogTask(params, config.actor, signal));
    },
  });

  pi.registerTool({
    name: "spool_report_worker_status",
    label: "Spool: Report Worker Status",
    description: "Report completion intent, blocked state, or retryable failure to the Worker Loop supervisor without changing Spool state.",
    parameters: WorkerStatusParams,
    async execute(_id, params) {
      return asToolResult(
        client.reportWorkerStatus(
          { ...params, agent_run_id: params.agent_run_id ?? config.agentRunId },
          config.actor,
          config.workerStatusPath,
        ),
      );
    },
  });
}
