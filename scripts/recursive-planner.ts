#!/usr/bin/env bun

import { mkdir, rm } from "node:fs/promises";

type TaskType = "bug_fix" | "feature" | "refactor" | "test" | "docs" | "infra" | "generic";
type Archetype = "frontend" | "backend" | "data" | "test" | "infra" | "review" | "security" | "generic";

type Footprint = {
  modifies: string[];
  creates: string[];
};

type RawPlannerTask = {
  title?: string;
  description?: string;
  type?: string;
  archetype?: string;
  acceptance_criteria?: string[];
  footprint?: {
    modifies?: string[];
    creates?: string[];
  };
  depends_on?: number[];
};

type TaskNode = {
  key: string;
  title: string;
  description: string;
  type: TaskType;
  archetype: Archetype;
  acceptanceCriteria: string[];
  footprint: Footprint;
  dependsOn: string[];
  depth: number;
  order: number;
};

type PlannerTask = {
  id: string;
  title: string;
  description: string;
  type: TaskType;
  archetype: Archetype;
  priority: number;
  dependsOn: string[];
  footprint: Footprint;
};

type LlmProvider = "auto" | "off" | "openai" | "claude" | "codex";
type TaskGranularity = "small" | "medium" | "large";

type Options = {
  goal: string;
  epicId: string;
  outputPath: string;
  dryRun: boolean;
  runImport: boolean;
  runValidate: boolean;
  maxDepth: number;
  maxTasks: number;
  model?: string;
  llmProvider: LlmProvider;
  granularity: TaskGranularity;
};

type PlannerRunStats = {
  llmProviderUsed: string | null;
  llmCalls: number;
};

const ALLOWED_TASK_TYPES: TaskType[] = ["bug_fix", "feature", "refactor", "test", "docs", "infra", "generic"];
const ALLOWED_ARCHETYPES: Archetype[] = ["frontend", "backend", "data", "test", "infra", "review", "security", "generic"];

const DEFAULT_OPENAI_MODEL = Bun.env.BACCHUS_PLANNER_MODEL ?? Bun.env.OPENAI_MODEL ?? "gpt-4.1-mini";

function printUsageAndExit(code = 1): never {
  const usage = `
Usage:
  bun scripts/recursive-planner.ts --goal "<goal>" [options]

Options:
  --goal <text>          Required planning goal.
  --epic-id <id>         Epic prefix for task IDs (default: PLAN).
  --output <path>        Output YAML path (default: .bacchus/tasks.yaml).
  --max-depth <n>        Recursive split depth (default: 2).
  --max-tasks <n>        Cap task count after recursion (default depends on granularity).
  --task-granularity <g> Task size target: small|medium|large (default: medium).
  --model <name>         Optional model override for selected provider.
  --llm-provider <p>     LLM backend: auto|claude|codex|openai|off (default: auto).
  --llm-mode <m>         Back-compat alias for provider mode: auto|off.
  --dry-run              Print YAML instead of writing file.
  --import               Run: bacchus task import --epic-id <EPIC_ID>.
  --validate             Run: bacchus task validate (implies --import).
  --help                 Show this help.

Env:
  OPENAI_API_KEY         Enables OpenAI API decomposition.
  OPENAI_BASE_URL        Optional base URL (default: https://api.openai.com/v1).
  BACCHUS_PLANNER_MODEL  Default OpenAI model override.
`.trim();

  console.log(usage);
  process.exit(code);
}

function parseArgs(argv: string[]): Options {
  let goal = "";
  let epicId = "PLAN";
  let outputPath = ".bacchus/tasks.yaml";
  let dryRun = false;
  let runImport = false;
  let runValidate = false;
  let maxDepth = 2;
  let maxTasks: number | null = null;
  let granularity: TaskGranularity = "medium";
  let model: string | undefined;
  let llmProvider: LlmProvider = "auto";

  const nextValue = (i: number, flag: string): string => {
    const v = argv[i + 1];
    if (!v || v.startsWith("--")) {
      throw new Error(`Missing value for ${flag}`);
    }
    return v;
  };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];

    if (arg === "--help" || arg === "-h") {
      printUsageAndExit(0);
    }

    if (arg === "--dry-run") {
      dryRun = true;
      continue;
    }
    if (arg === "--import") {
      runImport = true;
      continue;
    }
    if (arg === "--validate") {
      runValidate = true;
      runImport = true;
      continue;
    }

    if (arg.startsWith("--goal=")) {
      goal = arg.slice("--goal=".length).trim();
      continue;
    }
    if (arg.startsWith("--epic-id=")) {
      epicId = arg.slice("--epic-id=".length).trim();
      continue;
    }
    if (arg.startsWith("--output=")) {
      outputPath = arg.slice("--output=".length).trim();
      continue;
    }
    if (arg.startsWith("--max-depth=")) {
      maxDepth = Number.parseInt(arg.slice("--max-depth=".length), 10);
      continue;
    }
    if (arg.startsWith("--max-tasks=")) {
      maxTasks = Number.parseInt(arg.slice("--max-tasks=".length), 10);
      continue;
    }
    if (arg.startsWith("--task-granularity=")) {
      const value = arg.slice("--task-granularity=".length).trim();
      if (!isTaskGranularity(value)) {
        throw new Error(`Unsupported --task-granularity value: ${value}`);
      }
      granularity = value;
      continue;
    }
    if (arg.startsWith("--model=")) {
      model = arg.slice("--model=".length).trim();
      continue;
    }
    if (arg.startsWith("--llm-provider=")) {
      const mode = arg.slice("--llm-provider=".length).trim();
      if (isLlmProvider(mode)) {
        llmProvider = mode;
        continue;
      }
      throw new Error(`Unsupported --llm-provider value: ${mode}`);
    }
    if (arg.startsWith("--llm-mode=")) {
      const mode = arg.slice("--llm-mode=".length).trim();
      if (mode === "auto" || mode === "off") {
        llmProvider = mode;
        continue;
      }
      throw new Error(`Unsupported --llm-mode value: ${mode}`);
    }

    if (arg === "--goal") {
      goal = nextValue(i, "--goal").trim();
      i += 1;
      continue;
    }
    if (arg === "--epic-id") {
      epicId = nextValue(i, "--epic-id").trim();
      i += 1;
      continue;
    }
    if (arg === "--output") {
      outputPath = nextValue(i, "--output").trim();
      i += 1;
      continue;
    }
    if (arg === "--max-depth") {
      maxDepth = Number.parseInt(nextValue(i, "--max-depth"), 10);
      i += 1;
      continue;
    }
    if (arg === "--max-tasks") {
      maxTasks = Number.parseInt(nextValue(i, "--max-tasks"), 10);
      i += 1;
      continue;
    }
    if (arg === "--task-granularity") {
      const value = nextValue(i, "--task-granularity").trim();
      if (!isTaskGranularity(value)) {
        throw new Error(`Unsupported --task-granularity value: ${value}`);
      }
      granularity = value;
      i += 1;
      continue;
    }
    if (arg === "--model") {
      model = nextValue(i, "--model").trim();
      i += 1;
      continue;
    }
    if (arg === "--llm-provider") {
      const mode = nextValue(i, "--llm-provider").trim();
      if (!isLlmProvider(mode)) {
        throw new Error(`Unsupported --llm-provider value: ${mode}`);
      }
      llmProvider = mode;
      i += 1;
      continue;
    }
    if (arg === "--llm-mode") {
      const mode = nextValue(i, "--llm-mode").trim();
      if (mode !== "auto" && mode !== "off") {
        throw new Error(`Unsupported --llm-mode value: ${mode}`);
      }
      llmProvider = mode;
      i += 1;
      continue;
    }

    throw new Error(`Unknown argument: ${arg}`);
  }

  if (!goal) {
    throw new Error("--goal is required");
  }
  if (!Number.isFinite(maxDepth) || maxDepth < 0) {
    throw new Error("--max-depth must be >= 0");
  }
  const resolvedMaxTasks = maxTasks ?? defaultMaxTasksForGranularity(granularity);
  if (!Number.isFinite(resolvedMaxTasks) || resolvedMaxTasks < 1) {
    throw new Error("--max-tasks must be >= 1");
  }

  return {
    goal,
    epicId: normalizeEpicId(epicId),
    outputPath,
    dryRun,
    runImport,
    runValidate,
    maxDepth,
    maxTasks: resolvedMaxTasks,
    model,
    llmProvider,
    granularity,
  };
}

function isLlmProvider(value: string): value is LlmProvider {
  return value === "auto" || value === "off" || value === "openai" || value === "claude" || value === "codex";
}

function isTaskGranularity(value: string): value is TaskGranularity {
  return value === "small" || value === "medium" || value === "large";
}

function defaultMaxTasksForGranularity(granularity: TaskGranularity): number {
  if (granularity === "small") return 18;
  if (granularity === "large") return 10;
  return 16;
}

function normalizeEpicId(input: string): string {
  const cleaned = input
    .trim()
    .toUpperCase()
    .replace(/[^A-Z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return cleaned || "PLAN";
}

function normalizeType(value: string | undefined): TaskType | null {
  if (!value) return null;
  const normalized = value.trim().toLowerCase().replace("-", "_");
  return ALLOWED_TASK_TYPES.includes(normalized as TaskType) ? (normalized as TaskType) : null;
}

function normalizeArchetype(value: string | undefined): Archetype | null {
  if (!value) return null;
  const normalized = value.trim().toLowerCase().replace("-", "_");
  return ALLOWED_ARCHETYPES.includes(normalized as Archetype) ? (normalized as Archetype) : null;
}

function inferTypeAndArchetype(text: string): { type: TaskType; archetype: Archetype } {
  const t = text.toLowerCase();

  if (/(test|spec|coverage|fixture|mock|assert)/.test(t)) {
    return { type: "test", archetype: "test" };
  }
  if (/(docs|readme|comment|guide|changelog)/.test(t)) {
    return { type: "docs", archetype: "review" };
  }
  if (/(infra|ci|cd|deploy|docker|k8s|terraform|pipeline)/.test(t)) {
    return { type: "infra", archetype: "infra" };
  }
  if (/(refactor|cleanup|simplify|optimiz|restructur)/.test(t)) {
    return { type: "refactor", archetype: "backend" };
  }
  if (/(bug|fix|error|crash|regression|broken)/.test(t)) {
    return { type: "bug_fix", archetype: "backend" };
  }
  if (/(ui|ux|component|frontend|css|layout|responsive|a11y|accessibility)/.test(t)) {
    return { type: "feature", archetype: "frontend" };
  }
  if (/(migration|schema|etl|warehouse|sql|persist|storage|database|db)/.test(t)) {
    return { type: "feature", archetype: "data" };
  }
  if (/(security|auth|permission|rbac|secret|token|oauth)/.test(t)) {
    return { type: "feature", archetype: "security" };
  }
  return { type: "feature", archetype: "backend" };
}

function dedupeStrings(items: string[] | undefined): string[] {
  if (!items) return [];
  const seen = new Set<string>();
  const out: string[] = [];
  for (const item of items) {
    const trimmed = item.trim();
    if (!trimmed || seen.has(trimmed)) continue;
    seen.add(trimmed);
    out.push(trimmed);
  }
  return out;
}

function sanitizeRawTask(raw: RawPlannerTask, fallbackTitle: string): RawPlannerTask {
  const title = (raw.title ?? fallbackTitle).trim();
  const description = (raw.description ?? "").trim();
  const acceptance = dedupeStrings(raw.acceptance_criteria);
  const modifies = dedupeStrings(raw.footprint?.modifies);
  const creates = dedupeStrings(raw.footprint?.creates);
  const dependsOn = (raw.depends_on ?? []).filter((n) => Number.isInteger(n) && n >= 0);
  return {
    title,
    description,
    type: raw.type,
    archetype: raw.archetype,
    acceptance_criteria: acceptance,
    footprint: { modifies, creates },
    depends_on: dependsOn,
  };
}

function isBroadTask(node: TaskNode, granularity: TaskGranularity): { shouldSplit: boolean; reasons: string[] } {
  if (node.type === "test" || node.type === "docs" || node.type === "infra") {
    return { shouldSplit: false, reasons: [] };
  }

  const titleWords = node.title.split(/\s+/).filter(Boolean).length;
  const descWords = node.description.split(/\s+/).filter(Boolean).length;
  const artifactCount = node.footprint.modifies.length + node.footprint.creates.length;
  const joined = `${node.title} ${node.description}`.toLowerCase();

  const reasons: string[] = [];
  if (titleWords > 13) reasons.push("title-too-long");
  if (descWords > 70) reasons.push("description-too-long");
  if (/\b(and|plus|across|end-to-end|entire|all|complete|full)\b/.test(joined)) reasons.push("scope-broad");
  if (/[,:;].+[,:;]/.test(node.title)) reasons.push("multi-clause-title");
  if (artifactCount > 4) reasons.push("too-many-artifacts");

  const threshold = granularity === "small" ? 2 : granularity === "large" ? 4 : 3;
  return { shouldSplit: reasons.length >= threshold, reasons };
}

function mkNode(raw: RawPlannerTask, depth: number, order: number, key: string): TaskNode {
  const cleaned = sanitizeRawTask(raw, "Unnamed task");
  const typeHint = normalizeType(cleaned.type);
  const archHint = normalizeArchetype(cleaned.archetype);
  const inferred = inferTypeAndArchetype(`${cleaned.title} ${cleaned.description}`);
  return {
    key,
    title: cleaned.title || "Unnamed task",
    description: cleaned.description,
    type: typeHint ?? inferred.type,
    archetype: archHint ?? inferred.archetype,
    acceptanceCriteria: cleaned.acceptance_criteria ?? [],
    footprint: {
      modifies: cleaned.footprint?.modifies ?? [],
      creates: cleaned.footprint?.creates ?? [],
    },
    dependsOn: [],
    depth,
    order,
  };
}

function extractJsonObject(input: string): string | null {
  const start = input.indexOf("{");
  if (start < 0) return null;

  let depth = 0;
  let inString = false;
  let escaped = false;

  for (let i = start; i < input.length; i += 1) {
    const ch = input[i];

    if (inString) {
      if (escaped) {
        escaped = false;
      } else if (ch === "\\") {
        escaped = true;
      } else if (ch === "\"") {
        inString = false;
      }
      continue;
    }

    if (ch === "\"") {
      inString = true;
      continue;
    }
    if (ch === "{") {
      depth += 1;
      continue;
    }
    if (ch === "}") {
      depth -= 1;
      if (depth === 0) {
        return input.slice(start, i + 1);
      }
    }
  }
  return null;
}

function extractJsonArray(input: string): string | null {
  const start = input.indexOf("[");
  if (start < 0) return null;

  let depth = 0;
  let inString = false;
  let escaped = false;

  for (let i = start; i < input.length; i += 1) {
    const ch = input[i];

    if (inString) {
      if (escaped) {
        escaped = false;
      } else if (ch === "\\") {
        escaped = true;
      } else if (ch === "\"") {
        inString = false;
      }
      continue;
    }

    if (ch === "\"") {
      inString = true;
      continue;
    }
    if (ch === "[") {
      depth += 1;
      continue;
    }
    if (ch === "]") {
      depth -= 1;
      if (depth === 0) {
        return input.slice(start, i + 1);
      }
    }
  }

  return null;
}

async function decomposeWithLlm(
  prompt: string,
  options: Options,
  isRoot: boolean,
): Promise<{ tasks: RawPlannerTask[]; provider: string } | null> {
  const decompositionPrompt = buildDecompositionPrompt(
    prompt,
    isRoot,
    isRoot && !looksLikeSpec(prompt),
    options.granularity,
  );
  if (options.llmProvider === "off") return null;

  const providers: Array<Exclude<LlmProvider, "auto" | "off">> =
    options.llmProvider === "auto" ? ["claude", "openai"] : [options.llmProvider];

  for (const provider of providers) {
    let text: string | null = null;

    if (provider === "claude") {
      text = await decomposeWithClaude(decompositionPrompt, options);
    } else if (provider === "codex") {
      text = await decomposeWithCodex(decompositionPrompt, options);
    } else {
      text = await decomposeWithOpenAi(decompositionPrompt, options);
    }

    if (!text) continue;
    const tasks = parsePlannerTasksFromText(text);
    if (tasks && tasks.length >= 2) {
      return { tasks, provider };
    }
  }

  return null;
}

function buildDecompositionPrompt(
  prompt: string,
  isRoot: boolean,
  needsSpecTask: boolean,
  granularity: TaskGranularity,
): string {
  const rootTaskWindow =
    granularity === "small" ? "10-20" : granularity === "large" ? "4-8" : "6-14";
  const granularityDirective =
    granularity === "small"
      ? "Prefer finer-grained tasks and split by implementation slice (data, API, UI, and tests) when possible."
      : granularity === "large"
        ? "Prefer fewer broader tasks while keeping them executable."
        : "Balance detail and task count for practical multi-agent execution.";

  const rootRules = `
Root plan requirements:
- Return ${rootTaskWindow} tasks.
- Produce agent-friendly, atomic tasks (single primary deliverable each).
- Include setup/scaffolding and architecture/data-model tasks before implementation slices.
- ${needsSpecTask ? "Include a PRD/spec task first because no spec was provided." : "Treat provided goal as containing sufficient specification context."}
- Prefer parallelizable implementation slices with explicit dependencies.
- Include at least one dedicated integration/testing task near the end.
- Avoid mega tasks like "build entire app end-to-end".
- ${granularityDirective}
`.trim();

  const subtaskRules = `
Subtask decomposition requirements:
- Return 2-6 subtasks.
- Keep each subtask atomic and directly executable.
- Preserve parent intent while reducing scope.
`.trim();

  return `
Generate ${isRoot ? "a root execution plan" : "a subtask breakdown"}.

Rules:
- Keep each task independently executable and reviewable.
- Use allowed type values: ${ALLOWED_TASK_TYPES.join(", ")}.
- Use allowed archetype values: ${ALLOWED_ARCHETYPES.join(", ")}.
- "depends_on" must be an array of task indices (0-based) in the same response.
- Include a lightweight footprint guess only when obvious.
- Each task should be scoped so a single agent can finish it without additional decomposition.

${isRoot ? rootRules : subtaskRules}

Return strict JSON object:
{
  "tasks": [
    {
      "title": "string",
      "description": "string",
      "type": "feature",
      "archetype": "backend",
      "acceptance_criteria": ["string"],
      "footprint": {"modifies": ["path::symbol"], "creates": ["path/file.ts"]},
      "depends_on": [0]
    }
  ]
}

Work to decompose:
${prompt}
`.trim();
}

function parsePlannerTasksFromText(text: string): RawPlannerTask[] | null {
  const candidates: string[] = [];
  const trimmed = text.trim();
  if (trimmed) candidates.push(trimmed);

  const extractedObject = extractJsonObject(text);
  if (extractedObject) candidates.push(extractedObject);
  const extractedArray = extractJsonArray(text);
  if (extractedArray) candidates.push(extractedArray);

  for (const candidate of candidates) {
    let parsed: unknown;
    try {
      parsed = JSON.parse(candidate);
    } catch {
      continue;
    }

    let tasks: RawPlannerTask[] = [];
    if (Array.isArray(parsed)) {
      tasks = parsed as RawPlannerTask[];
    } else if (parsed && typeof parsed === "object") {
      const maybeObj = parsed as { tasks?: RawPlannerTask[]; plan?: { tasks?: RawPlannerTask[] } };
      tasks = maybeObj.tasks ?? maybeObj.plan?.tasks ?? [];
    }

    if (!Array.isArray(tasks)) return null;
    const cleaned = tasks
      .map((t, idx) => sanitizeRawTask(t, `Task ${idx + 1}`))
      .filter((t) => (t.title ?? "").trim().length > 0);
    if (cleaned.length < 2) return null;
    return cleaned.slice(0, 6);
  }

  return null;
}

function summarizeError(output: string): string {
  const line = output
    .split(/\r?\n/)
    .map((part) => part.trim())
    .find((part) => part.length > 0);
  return line ?? "unknown error";
}

async function decomposeWithOpenAi(prompt: string, options: Options): Promise<string | null> {
  const apiKey = Bun.env.OPENAI_API_KEY;
  if (!apiKey) return null;

  const baseUrl = (Bun.env.OPENAI_BASE_URL ?? "https://api.openai.com/v1").replace(/\/+$/, "");
  const url = `${baseUrl}/chat/completions`;
  const model = options.model ?? DEFAULT_OPENAI_MODEL;

  const response = await fetch(url, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify({
      model,
      temperature: 0.2,
      messages: [
        {
          role: "system",
          content: "You are a software planning assistant. Decompose work into atomic engineering tasks for Bacchus. Return JSON only.",
        },
        { role: "user", content: prompt },
      ],
    }),
  });

  if (!response.ok) {
    const body = await response.text();
    console.error(`OpenAI decomposition failed (${response.status}): ${body.slice(0, 200)}`);
    return null;
  }

  const payload = (await response.json()) as {
    choices?: Array<{ message?: { content?: string } }>;
  };
  return payload.choices?.[0]?.message?.content ?? null;
}

async function decomposeWithClaude(prompt: string, options: Options): Promise<string | null> {
  if (!Bun.which("claude")) return null;

  const schema = JSON.stringify({
    type: "object",
    properties: {
      tasks: {
        type: "array",
      },
    },
    required: ["tasks"],
  });

  const cmd = [
    "claude",
    "-p",
    "--no-session-persistence",
    "--permission-mode",
    "bypassPermissions",
    "--output-format",
    "text",
    "--json-schema",
    schema,
  ];
  if (options.model) {
    cmd.push("--model", options.model);
  }
  cmd.push(prompt);

  const proc = Bun.spawnSync({
    cmd,
    stdout: "pipe",
    stderr: "pipe",
  });

  if (proc.exitCode !== 0) {
    const stderr = proc.stderr.toString().trim();
    if (stderr) console.error(`Claude decomposition failed: ${summarizeError(stderr)}`);
    return null;
  }

  const stdout = proc.stdout.toString().trim();
  return stdout || null;
}

async function decomposeWithCodex(prompt: string, options: Options): Promise<string | null> {
  if (!Bun.which("codex")) return null;

  const base = `/tmp/bacchus-planner-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  const schemaPath = `${base}-schema.json`;
  const outputPath = `${base}-output.json`;
  const schema = {
    type: "object",
    properties: {
      tasks: {
        type: "array",
      },
    },
    required: ["tasks"],
  };

  await Bun.write(schemaPath, JSON.stringify(schema));

  const cmd = [
    "codex",
    "exec",
    "--ephemeral",
    "--skip-git-repo-check",
    "--output-schema",
    schemaPath,
    "-o",
    outputPath,
  ];
  if (options.model) {
    cmd.push("-m", options.model);
  }
  cmd.push(prompt);

  const proc = Bun.spawnSync({
    cmd,
    stdout: "pipe",
    stderr: "pipe",
  });

  let outputText = "";
  if (proc.exitCode === 0) {
    try {
      outputText = await Bun.file(outputPath).text();
    } catch {
      outputText = proc.stdout.toString().trim();
    }
  } else {
    const stderr = proc.stderr.toString().trim();
    if (stderr) console.error(`Codex decomposition failed: ${summarizeError(stderr)}`);
  }

  await rm(schemaPath, { force: true }).catch(() => {});
  await rm(outputPath, { force: true }).catch(() => {});

  return outputText || null;
}

function decomposeHeuristically(subject: string, isRoot: boolean, granularity: TaskGranularity): RawPlannerTask[] {
  const cleaned = subject.replace(/\s+/g, " ").trim();
  const capabilityLimit = granularity === "small" ? 12 : granularity === "large" ? 6 : 8;
  const capabilities = expandCompositeCapabilities(extractCapabilityList(cleaned), capabilityLimit);
  const hasSpec = looksLikeSpec(cleaned);

  if (isRoot) {
    const scope = scopePrefix(cleaned);

    const phased: RawPlannerTask[] = [];
    const idx = {
      spec: -1,
      scaffold: -1,
      architecture: -1,
      firstFeature: -1,
    };

    if (!hasSpec) {
      idx.spec = phased.length;
      phased.push({
        title: `Write PRD and technical spec for ${scope}`,
        description:
          "Define scope, user flows, data model, acceptance criteria, and non-functional requirements before implementation.",
        type: "docs",
        archetype: "review",
        acceptance_criteria: [
          "PRD captures user personas and core workflows.",
          "Technical spec includes architecture, data model, and API/component boundaries.",
        ],
        footprint: { modifies: [], creates: [] },
        depends_on: [],
      });
    }

    idx.scaffold = phased.length;
    phased.push({
      title: `Scaffold project foundation for ${scope}`,
      description:
        "Initialize project structure, lint/test tooling, env configuration, and local run workflow for agents.",
      type: "infra",
      archetype: "infra",
      acceptance_criteria: [
        "Project bootstraps cleanly for local development.",
        "Core scripts (dev/test/lint/build) are available.",
      ],
      footprint: { modifies: [], creates: [] },
      depends_on: idx.spec >= 0 ? [idx.spec] : [],
    });

    idx.architecture = phased.length;
    phased.push({
      title: `Define architecture and data model for ${scope}`,
      description: "Establish application boundaries, core entities, persistence strategy, and domain contracts.",
      type: "feature",
      archetype: "backend",
      acceptance_criteria: [
        "Architecture boundaries are documented and reflected in code structure.",
        "Data model supports required features and validation rules.",
      ],
      footprint: { modifies: [], creates: [] },
      depends_on: idx.spec >= 0 ? [idx.spec] : [],
    });

    if (capabilities.length >= 2 && capabilities.length <= 12) {
      idx.firstFeature = phased.length;
      const addUiTasks = granularity === "small" && isUiProductScope(scope);
      const implementationTasks: RawPlannerTask[] = [];

      for (let featureIdx = 0; featureIdx < capabilities.length; featureIdx += 1) {
        const capability = capabilities[featureIdx];
        const normalizedCapability = capability.replace(/\.$/, "");
        const inferred = inferTypeAndArchetype(`${scope} ${normalizedCapability}`);

        implementationTasks.push({
          title: `Implement ${normalizedCapability}`,
          description: `Build and verify the ${normalizedCapability} flow for ${scope}.`,
          type: inferred.type,
          archetype: inferred.archetype,
          acceptance_criteria: [`${capitalize(normalizedCapability)} flow works for happy and failure paths.`],
          footprint: { modifies: [], creates: [] },
          depends_on:
            featureIdx === 0
              ? [idx.scaffold, idx.architecture]
              : [idx.architecture],
        });
        const backendTaskAbsoluteIndex = phased.length + implementationTasks.length - 1;

        if (addUiTasks) {
          implementationTasks.push({
            title: `Implement UI for ${normalizedCapability}`,
            description: `Build user-facing interaction and validation for ${normalizedCapability}.`,
            type: "feature",
            archetype: "frontend",
            acceptance_criteria: [`UI interactions for ${normalizedCapability} work and are accessible.`],
            footprint: { modifies: [], creates: [] },
            depends_on: [backendTaskAbsoluteIndex],
          });
        }
      }

      phased.push(...implementationTasks);
      phased.push({
        title:
          granularity === "small"
            ? `Add integration, end-to-end, and regression tests for ${scope}`
            : `Add integration and end-to-end tests for ${scope}`,
        description: "Cover interactions across all implemented flows and key edge cases.",
        type: "test",
        archetype: "test",
        acceptance_criteria:
          granularity === "small"
            ? ["Automated tests cover all planned flows, regressions, and critical edge cases."]
            : ["Automated tests cover all planned flows together."],
        footprint: { modifies: [], creates: [] },
        depends_on: implementationTasks.map((_, i) => (idx.firstFeature < 0 ? i : idx.firstFeature + i)),
      });
      return phased;
    }

    idx.firstFeature = phased.length;
    phased.push({
      title: `Implement core functionality for ${scope}`,
      description: "Ship the primary user workflow as the first vertical slice.",
      type: "feature",
      archetype: inferTypeAndArchetype(cleaned).archetype,
      acceptance_criteria: ["Primary workflow executes successfully."],
      footprint: { modifies: [], creates: [] },
      depends_on: [idx.scaffold, idx.architecture],
    });
    phased.push({
      title: `Implement secondary and edge workflows for ${scope}`,
      description: "Add remaining behavior and robustness around validation and error handling.",
      type: "feature",
      archetype: inferTypeAndArchetype(cleaned).archetype,
      acceptance_criteria: ["Secondary and error workflows are complete."],
      footprint: { modifies: [], creates: [] },
      depends_on: [idx.firstFeature],
    });
    phased.push({
      title: `Add integration and end-to-end tests for ${scope}`,
      description: "Validate complete user journeys and data integrity across features.",
      type: "test",
      archetype: "test",
      acceptance_criteria: ["Integration tests cover major user journeys and edge cases."],
      footprint: { modifies: [], creates: [] },
      depends_on: [idx.firstFeature + 1],
    });
    return phased;
  }

  const inferred = inferTypeAndArchetype(cleaned);
  if (granularity === "small") {
    return [
      {
        title: `Implement core slice for ${cleaned}`,
        description: "Deliver a minimal vertical slice with core behavior.",
        type: inferred.type,
        archetype: inferred.archetype,
        acceptance_criteria: ["Subset behavior is complete and testable."],
        footprint: { modifies: [], creates: [] },
        depends_on: [],
      },
      {
        title: `Implement edge cases for ${cleaned}`,
        description: "Handle validation, failure paths, and resilience concerns.",
        type: inferred.type,
        archetype: inferred.archetype,
        acceptance_criteria: ["Edge-case behavior is correct and deterministic."],
        footprint: { modifies: [], creates: [] },
        depends_on: [0],
      },
      {
        title: `Add targeted tests for ${cleaned}`,
        description: "Add focused tests for core and edge behavior.",
        type: "test",
        archetype: "test",
        acceptance_criteria: ["Tests cover core plus edge behavior."],
        footprint: { modifies: [], creates: [] },
        depends_on: [1],
      },
    ];
  }

  return [
    {
      title: `Implement core slice for ${cleaned}`,
      description: "Deliver a minimal vertical slice with core behavior.",
      type: inferred.type,
      archetype: inferred.archetype,
      acceptance_criteria: ["Subset behavior is complete and testable."],
      footprint: { modifies: [], creates: [] },
      depends_on: [],
    },
    {
      title: `Complete integration and edge cases for ${cleaned}`,
      description: "Finalize remaining behavior and ensure robustness.",
      type: inferred.type,
      archetype: inferred.archetype,
      acceptance_criteria: ["Remaining behavior is implemented and validated."],
      footprint: { modifies: [], creates: [] },
      depends_on: [0],
    },
  ];
}

function scopePrefix(text: string): string {
  const scope = text
    .replace(/\b(with|including|include|supports?)\b[\s\S]*$/i, "")
    .replace(/^(implement|add|build|create|develop|ship)\s+/i, "")
    .trim();
  return scope || text;
}

function extractCapabilityList(text: string): string[] {
  const withMatch = text.match(/\b(?:with|including|include|supports?)\b([\s\S]*)$/i);
  if (!withMatch) return [];

  const tail = withMatch[1]
    .replace(/[.]/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  if (!tail) return [];

  const pieces = tail
    .split(/\s*(?:,|\band\b)\s*/i)
    .map((piece) => piece.trim())
    .filter((piece) => piece.length > 0 && piece.length <= 60)
    .filter((piece) => piece.split(/\s+/).length <= 7);

  const cleaned = dedupeStrings(
    pieces.map((piece) => piece.replace(/^(the|a|an)\s+/i, "").replace(/\bflow(s)?\b/i, "flow").trim()),
  );

  if (cleaned.length < 2) return [];
  return cleaned.slice(0, 6);
}

function expandCompositeCapabilities(capabilities: string[], limit: number): string[] {
  const expanded: string[] = [];

  for (const capability of capabilities) {
    const trimmed = capability.trim();
    if (!trimmed.includes("/")) {
      expanded.push(trimmed);
      continue;
    }

    const segments = trimmed.split("/").map((s) => s.trim()).filter(Boolean);
    if (segments.length < 2 || segments.length > 5) {
      expanded.push(trimmed);
      continue;
    }

    const last = segments[segments.length - 1];
    const noun = last.replace(/^(add|create|edit|update|delete|remove|view|list)\s+/i, "").trim();
    if (!noun) {
      expanded.push(trimmed);
      continue;
    }

    for (const seg of segments) {
      const hasNoun = /\s/.test(seg);
      if (hasNoun) {
        expanded.push(seg);
      } else {
        expanded.push(`${seg} ${noun}`.trim());
      }
    }
  }

  return dedupeStrings(expanded).slice(0, limit);
}

function looksLikeSpec(text: string): boolean {
  const t = text.toLowerCase();
  return (
    /\b(prd|product requirements|spec|specification|requirements doc|technical design)\b/.test(t) ||
    (/\b(acceptance criteria|non-functional|architecture|data model|api contract)\b/.test(t) &&
      t.split(/\s+/).length > 30)
  );
}

function isUiProductScope(text: string): boolean {
  const t = text.toLowerCase();
  return /\b(app|web|mobile|ui|frontend|dashboard|portal|site)\b/.test(t);
}

function capitalize(input: string): string {
  if (!input) return input;
  return input[0].toUpperCase() + input.slice(1);
}

async function decompose(
  subject: string,
  isRoot: boolean,
  options: Options,
  stats: PlannerRunStats,
): Promise<RawPlannerTask[]> {
  stats.llmCalls += 1;
  const llm = await decomposeWithLlm(subject, options, isRoot);
  if (llm && llm.tasks.length >= 2) {
    stats.llmProviderUsed = stats.llmProviderUsed ?? llm.provider;
    return llm.tasks;
  }
  return decomposeHeuristically(subject, isRoot, options.granularity);
}

function applyRootDependencies(nodes: TaskNode[], rawTasks: RawPlannerTask[]): void {
  for (let i = 0; i < nodes.length; i += 1) {
    const raw = sanitizeRawTask(rawTasks[i] ?? {}, nodes[i].title);
    const deps = (raw.depends_on ?? [])
      .filter((depIdx) => Number.isInteger(depIdx) && depIdx >= 0 && depIdx < nodes.length && depIdx !== i)
      .map((depIdx) => nodes[depIdx].key);
    nodes[i].dependsOn = dedupeStrings(deps);
  }

  // Heuristic: test/review/docs tasks should depend on an implementation task when no deps are provided.
  for (let i = 0; i < nodes.length; i += 1) {
    const node = nodes[i];
    if (node.dependsOn.length > 0) continue;
    if (!(node.type === "test" || node.type === "docs" || node.archetype === "review")) continue;
    for (let j = i - 1; j >= 0; j -= 1) {
      const candidate = nodes[j];
      if (candidate.type === "feature" || candidate.type === "bug_fix" || candidate.type === "refactor") {
        node.dependsOn = [candidate.key];
        break;
      }
    }
  }
}

function splitNode(tasks: TaskNode[], parentKey: string, childrenRaw: RawPlannerTask[], orderSeed: number): TaskNode[] {
  const idx = tasks.findIndex((t) => t.key === parentKey);
  if (idx < 0) return tasks;

  const parent = tasks[idx];
  const dependents = tasks.filter((t) => t.dependsOn.includes(parent.key));
  const childNodes: TaskNode[] = childrenRaw.slice(0, 6).map((raw, childIdx) => {
    const key = `${parent.key}.${childIdx + 1}`;
    return mkNode(raw, parent.depth + 1, orderSeed + childIdx, key);
  });

  if (childNodes.length < 2) return tasks;

  childNodes[0].dependsOn = dedupeStrings(parent.dependsOn);
  for (let i = 1; i < childNodes.length; i += 1) {
    childNodes[i].dependsOn = [childNodes[i - 1].key];
  }

  const lastChild = childNodes[childNodes.length - 1];
  for (const dep of dependents) {
    dep.dependsOn = dedupeStrings(dep.dependsOn.filter((k) => k !== parent.key).concat(lastChild.key));
  }

  tasks.splice(idx, 1, ...childNodes);
  return tasks;
}

function topologicalSort(nodes: TaskNode[]): TaskNode[] | null {
  const byKey = new Map<string, TaskNode>();
  for (const node of nodes) {
    byKey.set(node.key, node);
  }

  const indegree = new Map<string, number>();
  const outgoing = new Map<string, string[]>();
  for (const node of nodes) {
    indegree.set(node.key, 0);
    outgoing.set(node.key, []);
  }

  for (const node of nodes) {
    const deps = dedupeStrings(node.dependsOn.filter((d) => d !== node.key && byKey.has(d)));
    node.dependsOn = deps;
    for (const dep of deps) {
      indegree.set(node.key, (indegree.get(node.key) ?? 0) + 1);
      outgoing.get(dep)?.push(node.key);
    }
  }

  const queue = nodes
    .filter((node) => (indegree.get(node.key) ?? 0) === 0)
    .sort((a, b) => a.order - b.order)
    .map((n) => n.key);

  const result: TaskNode[] = [];
  while (queue.length > 0) {
    const key = queue.shift();
    if (!key) break;
    const node = byKey.get(key);
    if (!node) continue;
    result.push(node);

    for (const nextKey of outgoing.get(key) ?? []) {
      const nextDeg = (indegree.get(nextKey) ?? 0) - 1;
      indegree.set(nextKey, nextDeg);
      if (nextDeg === 0) {
        queue.push(nextKey);
        queue.sort((a, b) => (byKey.get(a)?.order ?? 0) - (byKey.get(b)?.order ?? 0));
      }
    }
  }

  if (result.length !== nodes.length) return null;
  return result;
}

function breakCyclesByLinearizing(nodes: TaskNode[]): TaskNode[] {
  const ordered = [...nodes].sort((a, b) => a.order - b.order);
  for (let i = 0; i < ordered.length; i += 1) {
    ordered[i].dependsOn = i === 0 ? [] : [ordered[i - 1].key];
  }
  return ordered;
}

function toFinalTasks(sorted: TaskNode[], epicId: string): PlannerTask[] {
  const idMap = new Map<string, string>();
  const width = Math.max(3, String(sorted.length).length);

  for (let i = 0; i < sorted.length; i += 1) {
    const id = `${epicId}-${String(i + 1).padStart(width, "0")}`;
    idMap.set(sorted[i].key, id);
  }

  return sorted.map((node, idx) => {
    const descriptionParts = [node.description.trim(), ...node.acceptanceCriteria.map((a) => `Acceptance: ${a}`)].filter(
      (s) => s.length > 0,
    );
    return {
      id: idMap.get(node.key) ?? `${epicId}-${String(idx + 1).padStart(width, "0")}`,
      title: node.title,
      description: descriptionParts.join(" "),
      type: node.type,
      archetype: node.archetype,
      priority: idx + 1,
      dependsOn: node.dependsOn.map((k) => idMap.get(k)).filter((v): v is string => Boolean(v)),
      footprint: node.footprint,
    };
  });
}

function yamlQuote(value: string): string {
  return `"${value.replaceAll("\\", "\\\\").replaceAll("\"", "\\\"")}"`;
}

function renderYaml(tasks: PlannerTask[]): string {
  const lines: string[] = [];
  lines.push("# Bacchus Task Configuration");
  lines.push("# Generated by scripts/recursive-planner.ts");
  lines.push("version: 1");
  lines.push("");
  lines.push("tasks:");

  for (const task of tasks) {
    lines.push(`  - id: ${task.id}`);
    lines.push(`    title: ${yamlQuote(task.title)}`);
    if (task.description) {
      lines.push("    description: |");
      for (const line of task.description.split("\n")) {
        lines.push(`      ${line}`);
      }
    }
    lines.push(`    type: ${task.type}`);
    lines.push(`    archetype: ${task.archetype}`);
    lines.push(`    priority: ${task.priority}`);
    lines.push("    status: open");
    if (task.dependsOn.length === 0) {
      lines.push("    depends_on: []");
    } else {
      lines.push(`    depends_on: [${task.dependsOn.join(", ")}]`);
    }
    lines.push("    footprint:");
    if (task.footprint.modifies.length === 0) {
      lines.push("      modifies: []");
    } else {
      lines.push("      modifies:");
      for (const mod of task.footprint.modifies) {
        lines.push(`        - ${yamlQuote(mod)}`);
      }
    }
    if (task.footprint.creates.length === 0) {
      lines.push("      creates: []");
    } else {
      lines.push("      creates:");
      for (const create of task.footprint.creates) {
        lines.push(`        - ${yamlQuote(create)}`);
      }
    }
    lines.push("");
  }

  return `${lines.join("\n").trimEnd()}\n`;
}

function runBacchus(args: string[]): void {
  const proc = Bun.spawnSync({
    cmd: ["bacchus", ...args],
    stdout: "pipe",
    stderr: "pipe",
  });

  const stdout = proc.stdout.toString().trim();
  const stderr = proc.stderr.toString().trim();

  if (stdout) console.log(stdout);
  if (stderr) console.error(stderr);

  if (proc.exitCode !== 0) {
    throw new Error(`Command failed: bacchus ${args.join(" ")}`);
  }
}

async function ensureParentDir(path: string): Promise<void> {
  const dir = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : ".";
  if (!dir || dir === ".") return;
  await mkdir(dir, { recursive: true });
}

async function main(): Promise<void> {
  const options = parseArgs(Bun.argv.slice(2));
  const stats: PlannerRunStats = {
    llmProviderUsed: null,
    llmCalls: 0,
  };

  let keyCounter = 0;
  let orderCounter = 0;
  const nextKey = (): string => {
    keyCounter += 1;
    return `t${keyCounter}`;
  };
  const nextOrder = (): number => {
    orderCounter += 1;
    return orderCounter * 10;
  };

  const rootRawTasks = await decompose(options.goal, true, options, stats);
  let tasks = rootRawTasks.map((raw) => mkNode(raw, 0, nextOrder(), nextKey()));
  applyRootDependencies(tasks, rootRawTasks);

  let splitPasses = 0;
  while (tasks.length < options.maxTasks) {
    let didSplit = false;
    const snapshot = [...tasks];

    for (const node of snapshot) {
      if (tasks.length >= options.maxTasks) break;
      if (node.depth >= options.maxDepth) continue;

      const splitDecision = isBroadTask(node, options.granularity);
      if (!splitDecision.shouldSplit) continue;

      const subTasks = await decompose(`${node.title}\n${node.description}`, false, options, stats);
      if (subTasks.length < 2) continue;

      const clipped = subTasks.slice(0, Math.max(2, Math.min(6, options.maxTasks - tasks.length + 1)));
      tasks = splitNode(tasks, node.key, clipped, nextOrder());
      didSplit = true;
    }

    if (!didSplit) break;
    splitPasses += 1;
    if (splitPasses > options.maxDepth + 2) break;
  }

  if (tasks.length > options.maxTasks) {
    tasks = tasks.slice(0, options.maxTasks);
  }

  let sorted = topologicalSort(tasks);
  if (!sorted) {
    console.error("Dependency cycle detected from planner output; linearizing dependencies.");
    const linear = breakCyclesByLinearizing(tasks);
    sorted = topologicalSort(linear);
  }

  if (!sorted) {
    throw new Error("Failed to produce a valid task DAG.");
  }

  const finalTasks = toFinalTasks(sorted, options.epicId);
  const yaml = renderYaml(finalTasks);

  if (options.dryRun) {
    console.log(yaml);
  } else {
    await ensureParentDir(options.outputPath);
    await Bun.write(options.outputPath, yaml);
    console.log(`Wrote ${finalTasks.length} tasks to ${options.outputPath}`);
  }

  if (options.runImport) {
    runBacchus(["task", "import", "--epic-id", options.epicId]);
  }
  if (options.runValidate) {
    runBacchus(["task", "validate"]);
  }

  console.log(
    JSON.stringify(
      {
        success: true,
        epic_id: options.epicId,
        task_granularity: options.granularity,
        tasks: finalTasks.length,
        recursive_splits: splitPasses,
        used_llm: stats.llmProviderUsed !== null,
        llm_provider: stats.llmProviderUsed,
        llm_calls: stats.llmCalls,
      },
      null,
      2,
    ),
  );
}

main().catch((error) => {
  console.error(String(error instanceof Error ? error.message : error));
  process.exit(1);
});
