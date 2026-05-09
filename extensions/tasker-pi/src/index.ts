import { Type } from "typebox";
import { TaskerClient, configFromEnv } from "./client";
import type { ExtensionAPI, TaskerToolResult } from "./types";

const Identifier = Type.String({ description: "Task Identifier, such as TASKER-1" });
const WorkpadParams = Type.Object({ identifier: Identifier, body: Type.String() });
const StatusParams = Type.Object({
  identifier: Identifier,
  position: Type.Number({ description: "1-based requirement position" }),
  status: Type.String(),
  waiver_reason: Type.Optional(Type.String()),
});
const ChildTaskParams = Type.Object({
  parent_identifier: Identifier,
  title: Type.String(),
  brief: Type.String(),
  acceptance_criteria: Type.Array(Type.String()),
  validation_items: Type.Array(Type.String()),
  priority: Type.Optional(Type.String()),
  state: Type.Optional(Type.String()),
  tags: Type.Optional(Type.Array(Type.String())),
  review_required: Type.Optional(Type.Boolean()),
  blocks_parent: Type.Optional(Type.Boolean()),
});
const TransitionParams = Type.Object({
  identifier: Identifier,
  to_state: Type.String(),
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
    name: "tasker_update_workpad",
    label: "Tasker: Update Workpad",
    description: "Replace the Task's singleton Workpad Note body.",
    parameters: WorkpadParams,
    async execute(_id, params, signal) {
      return asToolResult(await client.updateWorkpad(params.identifier, config.actor, params.body, signal));
    },
  });

  pi.registerTool({
    name: "tasker_set_acceptance_criterion_status",
    label: "Tasker: Set Acceptance Criterion Status",
    description: "Set an Acceptance Criterion status by 1-based position.",
    parameters: StatusParams,
    async execute(_id, params, signal) {
      return asToolResult(await client.setAcceptanceCriterionStatus(params, config.actor, signal));
    },
  });

  pi.registerTool({
    name: "tasker_set_validation_item_status",
    label: "Tasker: Set Validation Item Status",
    description: "Set a Validation Item status by 1-based position.",
    parameters: StatusParams,
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
}
