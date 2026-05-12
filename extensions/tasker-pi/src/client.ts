import { execFileSync } from "node:child_process";
import { appendFileSync, existsSync } from "node:fs";
import type { Actor, CreateChildTaskInput, DelegationTaskDraftInput, RefineBacklogTaskInput, RequirementStatusInput, ReviewDecisionInput, TaskerExtensionConfig, WorkerStatusReportInput } from "./types";

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

  async getTaskContextBundle(identifier: string, signal?: AbortSignal): Promise<unknown> {
    const bundle = await this.request("GET", `/tasks/${encodeURIComponent(identifier)}/context-bundle`, undefined, signal);
    validateTaskContextBundle(bundle);
    return bundle;
  }

  updateWorkpad(identifier: string, actor: Actor, body: string, signal?: AbortSignal): Promise<unknown> {
    return this.request("PUT", `/tasks/${encodeURIComponent(identifier)}/workpad`, { actor, body }, signal);
  }

  async appendWorkpad(identifier: string, actor: Actor, body: string, separator = "\n\n", signal?: AbortSignal): Promise<unknown> {
    const task = await this.getTask(identifier, signal);
    const existingBody = workpadBody(task);
    const nextBody = existingBody.length === 0 ? body : `${existingBody}${separator}${body}`;
    return this.updateWorkpad(identifier, actor, nextBody, signal);
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
      {
        actor,
        status: input.status,
        waiver_reason: input.waiver_reason ?? null,
        validated_base_commit: input.validated_base_commit ?? null,
      },
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

  async requestTransition(identifier: string, toState: string, actor: Actor, agentRunId?: string, signal?: AbortSignal): Promise<unknown> {
    let preflightWarning: string | undefined;
    if (toState === "integrating") {
      const task = await this.getTask(identifier, signal);
      const inspection = inspectPreIntegratingLocalWorktree(task, identifier);
      if (actor.kind === "worker_agent") {
        inspection.rejectIfNotReady();
      } else {
        preflightWarning = inspection.operatorWarning();
      }
    }
    const detail = await this.request("POST", `/tasks/${encodeURIComponent(identifier)}/transition`, {
      actor,
      to_state: toState,
      agent_run_id: agentRunId ?? null,
    }, signal);
    return preflightWarning === undefined ? detail : { detail, preflight_warning: preflightWarning };
  }

  recordReviewDecision(input: ReviewDecisionInput, actor: Actor, signal?: AbortSignal): Promise<unknown> {
    return this.request("POST", `/tasks/${encodeURIComponent(input.identifier)}/review-decision`, {
      actor,
      decision: input.decision,
      feedback: input.feedback ?? null,
    }, signal);
  }

  createDelegatedRootTask(input: DelegationTaskDraftInput, actor: Actor, signal?: AbortSignal): Promise<unknown> {
    return this.request("POST", "/tasks/delegated-root", {
      actor,
      draft: {
        queue_key: input.queue_key,
        title: input.title,
        brief: input.brief,
        priority: input.priority ?? "normal",
        initial_state: input.initial_state ?? "backlog",
        review_required: input.review_required ?? false,
        tags: input.tags ?? [],
        conflict_hints: input.conflict_hints ?? [],
        blocking_task_identifiers: input.blocking_task_identifiers ?? [],
        acceptance_criteria: input.acceptance_criteria ?? [],
        validation_items: input.validation_items ?? [],
      },
    }, signal);
  }

  refineBacklogTask(input: RefineBacklogTaskInput, actor: Actor, signal?: AbortSignal): Promise<unknown> {
    return this.request("POST", `/tasks/${encodeURIComponent(input.identifier)}/refine`, {
      actor,
      refinement: {
        title: input.title ?? null,
        brief: input.brief ?? null,
        priority: input.priority ?? null,
        target_state: input.target_state ?? null,
        review_required: input.review_required ?? null,
        tags: input.tags ?? null,
        conflict_hints: input.conflict_hints ?? null,
        blocking_task_identifiers: input.blocking_task_identifiers ?? null,
        acceptance_criteria: input.acceptance_criteria ?? [],
        validation_items: input.validation_items ?? [],
      },
    }, signal);
  }

  reportWorkerStatus(input: WorkerStatusReportInput, actor: Actor, workerStatusPath?: string): unknown {
    const report = {
      tasker_worker_status: true,
      task_identifier: input.identifier,
      agent_run_id: input.agent_run_id ?? null,
      status: input.status,
      message: input.message ?? null,
      actor,
      reported_at: new Date().toISOString(),
    };
    if (workerStatusPath) {
      appendFileSync(workerStatusPath, `${JSON.stringify(report)}\n`);
    }
    return report;
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

interface PreIntegratingInspection {
  identifier: string;
  localWorktree?: string;
  taskBranch?: string;
  statusSummary?: string;
  issue?: string;
  rejectIfNotReady(): void;
  operatorWarning(): string | undefined;
}

function inspectPreIntegratingLocalWorktree(task: unknown, identifier: string): PreIntegratingInspection {
  const localWorktree = taskLink(task, "local_worktree");
  const taskBranch = taskLink(task, "task_branch");
  const inspection: PreIntegratingInspection = {
    identifier,
    localWorktree,
    taskBranch,
    rejectIfNotReady() {
      if (this.issue) throw new Error(preIntegratingGuidance(this, this.issue));
    },
    operatorWarning() {
      return this.issue === undefined
        ? undefined
        : `${preIntegratingGuidance(this, this.issue)}; operator transition may continue for repair flexibility, but Worker Agents must commit intended changes on the Task Branch and verify a clean Local Worktree before requesting Integrating`;
    },
  };
  if (!localWorktree) {
    inspection.issue = "missing Local Worktree Task Link";
    return inspection;
  }
  if (!taskBranch) {
    inspection.issue = "missing Task Branch Task Link";
    return inspection;
  }
  if (!existsSync(localWorktree)) {
    inspection.issue = "Local Worktree path does not exist";
    return inspection;
  }
  try {
    const status = gitOutput(localWorktree, ["status", "--porcelain"]);
    inspection.statusSummary = condenseGitStatusSummary(status);
    if (status.trim().length > 0) inspection.issue = "Local Worktree has uncommitted changes";
  } catch (error) {
    inspection.issue = `could not inspect Local Worktree git status: ${error instanceof Error ? error.message : String(error)}`;
    return inspection;
  }
  try {
    const branch = gitOutput(localWorktree, ["rev-parse", "--abbrev-ref", "HEAD"]).trim();
    if (branch !== taskBranch) inspection.issue = `Local Worktree is on branch ${branch}, expected Task Branch ${taskBranch}`;
  } catch (error) {
    inspection.issue = `could not inspect Local Worktree branch: ${error instanceof Error ? error.message : String(error)}`;
  }
  return inspection;
}

function taskLink(task: unknown, kind: string): string | undefined {
  if (!task || typeof task !== "object" || !("task_links" in task)) return undefined;
  const links = (task as { task_links?: unknown }).task_links;
  if (!Array.isArray(links)) return undefined;
  const link = links.find((candidate) => candidate && typeof candidate === "object" && (candidate as { kind?: unknown }).kind === kind);
  const target = link && typeof link === "object" ? (link as { target?: unknown }).target : undefined;
  return typeof target === "string" ? target : undefined;
}

function preIntegratingGuidance(inspection: PreIntegratingInspection, issue: string): string {
  return `Local Worktree pre-Integrating check failed for Task ${inspection.identifier}: ${issue}. Local Worktree: ${inspection.localWorktree ?? "missing Local Worktree Task Link"}; Task Branch: ${inspection.taskBranch ?? "missing Task Branch Task Link"}; git status summary: ${inspection.statusSummary ?? "unavailable"}. Commit intended changes on the Task Branch, verify the Local Worktree is clean, then request Integrating again.`;
}

function condenseGitStatusSummary(status: string): string {
  const lines = status.split(/\r?\n/).filter((line) => line.length > 0);
  if (lines.length === 0) return "clean";
  const shown = lines.slice(0, 12).map((line) => line.trimEnd()).join("; ");
  const remaining = lines.length - 12;
  return remaining > 0 ? `${shown}; ... and ${remaining} more` : shown;
}

function gitOutput(cwd: string, args: string[]): string {
  return execFileSync("git", ["-C", cwd, ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
}

function workpadBody(task: unknown): string {
  if (!task || typeof task !== "object" || !("workpad_note" in task)) return "";
  const note = (task as { workpad_note?: unknown }).workpad_note;
  if (!note || typeof note !== "object" || !("body" in note)) return "";
  const body = (note as { body?: unknown }).body;
  return typeof body === "string" ? body : "";
}

function validateTaskContextBundle(bundle: unknown): void {
  if (!bundle || typeof bundle !== "object") throw new Error("Task context bundle must be an object");
  const value = bundle as Record<string, unknown>;
  if (!value.task || typeof value.task !== "object") throw new Error("Task context bundle missing task");
  const taskDetail = value.task as Record<string, unknown>;
  for (const field of [
    "acceptance_criteria",
    "validation_items",
    "task_links",
    "conflict_hints",
    "blocking_tasks",
    "blocked_tasks",
  ]) {
    if (!Array.isArray(taskDetail[field])) throw new Error(`Task context bundle missing task.${field}`);
  }
  if (!value.queue || typeof value.queue !== "object") throw new Error("Task context bundle missing queue");
  if (!value.local_workflow || typeof value.local_workflow !== "object") {
    throw new Error("Task context bundle missing local_workflow");
  }
  if (!value.advisory_hints || typeof value.advisory_hints !== "object") {
    throw new Error("Task context bundle missing advisory_hints");
  }
  const advisoryHints = value.advisory_hints as Record<string, unknown>;
  if (!Array.isArray(advisoryHints.task_conflict_hints)) {
    throw new Error("Task context bundle missing advisory Task Conflict Hints");
  }
  if (!Array.isArray(advisoryHints.likely_files_or_paths)) {
    throw new Error("Task context bundle missing likely files/path guidance");
  }
  if (!Array.isArray(value.agent_runs)) throw new Error("Task context bundle missing agent_runs");
  rejectForbiddenContextKeys(value);
}

function rejectForbiddenContextKeys(value: unknown): void {
  if (!value || typeof value !== "object") return;
  for (const [key, child] of Object.entries(value)) {
    const normalized = key.toLowerCase();
    if (normalized === "raw_json" || normalized.includes("transcript")) {
      throw new Error(`Task context bundle contains forbidden field ${key}`);
    }
    rejectForbiddenContextKeys(child);
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
    workerStatusPath: env.TASKER_WORKER_STATUS_PATH,
  };
}
