export interface Actor {
  kind: "operator" | "delegating_agent" | "worker_agent" | "review_agent" | string;
  id: string;
  display_name: string;
}

export interface TaskerExtensionConfig {
  apiUrl: string;
  apiToken: string;
  actor: Actor;
  agentRunId?: string;
  workerStatusPath?: string;
}

export interface TaskerToolResult {
  content: Array<{ type: "text"; text: string }>;
  details: unknown;
}

export interface ExtensionAPI {
  registerTool(tool: {
    name: string;
    label?: string;
    description: string;
    parameters: unknown;
    execute: (
      toolCallId: string,
      params: any,
      signal: AbortSignal,
      onUpdate?: unknown,
      ctx?: unknown,
    ) => Promise<TaskerToolResult>;
  }): void;
}

export interface RequirementStatusInput {
  identifier: string;
  position: number;
  status: string;
  waiver_reason?: string;
  validated_base_commit?: string;
}

export interface ReviewDecisionInput {
  identifier: string;
  decision: "approve" | "rework";
  feedback?: string;
}

export interface CreateChildTaskInput {
  parent_identifier: string;
  title: string;
  brief: string;
  acceptance_criteria: string[];
  validation_items: string[];
  priority?: "urgent" | "high" | "normal" | "low";
  state?: "backlog" | "ready";
  tags?: string[];
  review_required?: boolean;
  blocks_parent?: boolean;
}

export type WorkerStatus = "completion_intent" | "blocked" | "retryable_failure";

export interface WorkerStatusReportInput {
  identifier: string;
  status: WorkerStatus;
  message?: string;
  agent_run_id?: string;
}
