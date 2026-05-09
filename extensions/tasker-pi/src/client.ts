import type { Actor, CreateChildTaskInput, RequirementStatusInput, TaskerExtensionConfig } from "./types";

export class TaskerClient {
  private readonly apiUrl: string;
  private readonly apiToken: string;

  constructor(config: Pick<TaskerExtensionConfig, "apiUrl" | "apiToken">) {
    this.apiUrl = config.apiUrl.replace(/\/+$/, "");
    this.apiToken = config.apiToken;
  }

  getTask(identifier: string, signal?: AbortSignal): Promise<unknown> {
    return this.request("GET", `/tasks/${encodeURIComponent(identifier)}`, undefined, signal);
  }

  updateWorkpad(identifier: string, actor: Actor, body: string, signal?: AbortSignal): Promise<unknown> {
    return this.request("PUT", `/tasks/${encodeURIComponent(identifier)}/workpad`, { actor, body }, signal);
  }

  setAcceptanceCriterionStatus(input: RequirementStatusInput, actor: Actor, signal?: AbortSignal): Promise<unknown> {
    return this.request(
      "PUT",
      `/tasks/${encodeURIComponent(input.identifier)}/acceptance-criteria/${input.position}/status`,
      { actor, status: input.status, waiver_reason: input.waiver_reason ?? null },
      signal,
    );
  }

  setValidationItemStatus(input: RequirementStatusInput, actor: Actor, signal?: AbortSignal): Promise<unknown> {
    return this.request(
      "PUT",
      `/tasks/${encodeURIComponent(input.identifier)}/validation-items/${input.position}/status`,
      { actor, status: input.status, waiver_reason: input.waiver_reason ?? null },
      signal,
    );
  }

  createChildTask(input: CreateChildTaskInput, actor: Actor, signal?: AbortSignal): Promise<unknown> {
    return this.request("POST", `/tasks/${encodeURIComponent(input.parent_identifier)}/child-tasks`, {
      actor,
      task: {
        title: input.title,
        brief: input.brief,
        priority: input.priority ?? "normal",
        state: input.state ?? "backlog",
        review_required: input.review_required ?? false,
        acceptance_criteria: input.acceptance_criteria,
        validation_items: input.validation_items,
        tags: input.tags ?? [],
        blocks_parent: input.blocks_parent ?? false,
      },
    }, signal);
  }

  requestTransition(identifier: string, toState: string, actor: Actor, agentRunId?: string, signal?: AbortSignal): Promise<unknown> {
    return this.request("POST", `/tasks/${encodeURIComponent(identifier)}/transition`, {
      actor,
      to_state: toState,
      agent_run_id: agentRunId ?? null,
    }, signal);
  }

  private async request(method: string, path: string, body?: unknown, signal?: AbortSignal): Promise<unknown> {
    const response = await fetch(`${this.apiUrl}${path}`, {
      method,
      signal,
      headers: {
        authorization: `Bearer ${this.apiToken}`,
        ...(body === undefined ? {} : { "content-type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (!response.ok) {
      const text = await response.text();
      throw new Error(`Tasker API ${method} ${path} failed (${response.status}): ${text}`);
    }
    if (response.status === 204) return null;
    return response.json();
  }
}

export function configFromEnv(env: Record<string, string | undefined> = process.env): TaskerExtensionConfig {
  const apiUrl = env.TASKER_API_URL ?? "http://127.0.0.1:3000";
  const apiToken = env.TASKER_API_TOKEN;
  if (!apiToken) throw new Error("TASKER_API_TOKEN is required for the Tasker Pi Extension");
  const actorId = env.TASKER_ACTOR_ID ?? env.TASKER_ACTOR ?? "pi-worker";
  return {
    apiUrl,
    apiToken,
    actor: {
      kind: env.TASKER_ACTOR_KIND ?? "worker_agent",
      id: actorId,
      display_name: env.TASKER_ACTOR_DISPLAY_NAME ?? actorId,
    },
    agentRunId: env.TASKER_AGENT_RUN_ID,
  };
}
