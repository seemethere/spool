import { Type } from "typebox";
import { TaskerClient, configFromEnv } from "./client";
import type { ExtensionAPI, TaskerToolResult } from "./types";

const Identifier = Type.String({ description: "Task Identifier, such as TASKER-1" });
const CriterionStatus = Type.Union([Type.Literal("pending"), Type.Literal("satisfied"), Type.Literal("waived")]);
const ValidationStatus = Type.Union([
  Type.Literal("pending"),
  Type.Literal("passed"),
  Type.Literal("failed"),
  Type.Literal("waived"),
]);
const Priority = Type.Union([Type.Literal("urgent"), Type.Literal("high"), Type.Literal("normal"), Type.Literal("low")]);
const ChildTaskState = Type.Union([Type.Literal("backlog"), Type.Literal("ready")]);
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
const TransitionParams = Type.Object({
  identifier: Identifier,
  to_state: TaskState,
  agent_run_id: Type.Optional(Type.String()),
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

function asToolResult(details: unknown): TaskerToolResult {
  return {
    content: [{ type: "text", text: JSON.stringify(details, null, 2) }],
    details,
  };
}

export default function registerTaskerExtension(pi: ExtensionAPI) {
  const config = configFromEnv();
  const client = new TaskerClient(config);

  pi.registerTool({
    name: "tasker_get_task",
    label: "Tasker: Get Task",
    description: "Fetch full Tasker Task context by Task Identifier.",
    parameters: Type.Object({ identifier: Identifier }),
    async execute(_id, params, signal) {
      return asToolResult(await client.getTask(params.identifier, signal));
    },
  });

  pi.registerTool({
    name: "tasker_get_task_context_bundle",
    label: "Tasker: Get Task Context Bundle",
    description:
      "Fetch the read-only Tasker-owned run-start context bundle for a Worker Agent without raw transcripts, raw launcher payloads, secrets, or unrelated queue data.",
    parameters: Type.Object({ identifier: Identifier }),
    async execute(_id, params, signal) {
      return asToolResult(await client.getTaskContextBundle(params.identifier, signal));
    },
  });

  pi.registerTool({
    name: "tasker_update_workpad",
    label: "Tasker: Update Workpad",
    description: "Replace the Task's singleton Workpad Note body.",
    parameters: WorkpadParams,
    async execute(_id, params, signal) {
      return asToolResult(await client.updateWorkpad(params.identifier, config.actor, params.body, signal));
    },
  });

  pi.registerTool({
    name: "tasker_append_workpad",
    label: "Tasker: Append Workpad",
    description: "Append Markdown to the current Workpad Note without manually replacing the whole note.",
    parameters: AppendWorkpadParams,
    async execute(_id, params, signal) {
      return asToolResult(
        await client.appendWorkpad(params.identifier, config.actor, params.body, params.separator ?? "\n\n", signal),
      );
    },
  });

  pi.registerTool({
    name: "tasker_set_acceptance_criterion_status",
    label: "Tasker: Set Acceptance Criterion Status",
    description: "Set an Acceptance Criterion status by 1-based position.",
    parameters: AcceptanceCriterionStatusParams,
    async execute(_id, params, signal) {
      return asToolResult(await client.setAcceptanceCriterionStatus(params, config.actor, signal));
    },
  });

  pi.registerTool({
    name: "tasker_set_validation_item_status",
    label: "Tasker: Set Validation Item Status",
    description: "Set a Validation Item status by 1-based position.",
    parameters: ValidationItemStatusParams,
    async execute(_id, params, signal) {
      return asToolResult(await client.setValidationItemStatus(params, config.actor, signal));
    },
  });

  pi.registerTool({
    name: "tasker_create_child_task",
    label: "Tasker: Create Child Task",
    description: "Create a same-queue Child Task from the current Task context.",
    parameters: ChildTaskParams,
    async execute(_id, params, signal) {
      return asToolResult(await client.createChildTask(params, config.actor, signal));
    },
  });

  pi.registerTool({
    name: "tasker_request_transition",
    label: "Tasker: Request State Transition",
    description: "Request a normal Task State Transition through Tasker gates.",
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
    name: "tasker_report_worker_status",
    label: "Tasker: Report Worker Status",
    description: "Report completion intent, blocked state, or retryable failure to the Worker Loop supervisor without changing Tasker state.",
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
