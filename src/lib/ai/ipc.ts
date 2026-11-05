/**
 * AI search IPC + provider presets.
 *
 * Keys live in `<app_config>/ai.json` (plaintext, like `navidrome.json`) and
 * never leave the Rust side except in the settings editor itself. All
 * providers except Anthropic speak the OpenAI `/chat/completions` shape;
 * Anthropic gets its own adapter in `navinamp-llm`.
 */

import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import type { SmartViewDef, Track } from "../library/ipc";

export type AiProviderKind = "openAiCompatible" | "anthropic";

export interface AiConfig {
  enabled: boolean;
  /** Preset id (`openrouter`, `openai`, `anthropic`, `custom`, …). */
  preset: string;
  provider: AiProviderKind;
  baseUrl: string;
  model: string;
  /** Embedding model for SEMANTIC search; empty = not configured. */
  embeddingModel: string;
  apiKey: string;
  timeoutSecs: number;
  /** Required for Discogs catalog lookups (free personal access token). */
  discogsToken: string;
}

export interface AiPreset {
  id: string;
  label: string;
  provider: AiProviderKind;
  baseUrl: string;
  model: string;
  /** Current model ids for the picker; the live `/models` fetch can extend it. */
  models: string[];
  /** Suggested embeddings model for SEMANTIC search (editable). */
  embeddingModel?: string;
  hint?: string;
}

/**
 * Provider presets. Base URL/model stay editable; subscriptions sold for a
 * chat app usually do not include API access, so a normal API key (or an
 * OpenRouter key) is required.
 */
export const AI_PRESETS: AiPreset[] = [
  {
    id: "openrouter",
    label: "OpenRouter (many models)",
    provider: "openAiCompatible",
    baseUrl: "https://openrouter.ai/api/v1",
    model: "deepseek/deepseek-v4.1-flash-1017",
    models: [
      "deepseek/deepseek-v4.1-flash-1017",
      "z-ai/glm-5.3-flash",
    ],
    hint: "one key for OpenAI, Claude, Gemini, DeepSeek, GLM, Kimi, MiniMax… use REFRESH for the full list",
  },
  {
    id: "openai",
    label: "OpenAI",
    provider: "openAiCompatible",
    baseUrl: "https://api.openai.com/v1",
    model: "gpt-5.2",
    models: ["gpt-5.2", "gpt-5.1", "gpt-5"],
    embeddingModel: "text-embedding-3-small",
  },
  {
    id: "anthropic",
    label: "Anthropic (Claude)",
    provider: "anthropic",
    baseUrl: "https://api.anthropic.com",
    model: "claude-sonnet-5",
    models: ["claude-sonnet-5"],
  },
  {
    id: "deepseek",
    label: "DeepSeek",
    provider: "openAiCompatible",
    baseUrl: "https://api.deepseek.com",
    model: "deepseek-flash",
    models: ["deepseek-flash", "deepseek-v4-pro"],
    hint: "deepseek-flash serves DeepSeek V4.1 Flash (1017 snapshot)",
  },
  {
    id: "glm",
    label: "Zhipu GLM",
    provider: "openAiCompatible",
    baseUrl: "https://api.z.ai/api/paas/v4",
    model: "glm-5.3-flash",
    models: ["glm-5.3-flash", "glm-5.3", "glm-5.3-highspeed"],
    hint: "Z.ai platform; use the bigmodel.cn endpoint as a custom URL for the China platform",
  },
  {
    id: "kimi",
    label: "Moonshot Kimi",
    provider: "openAiCompatible",
    baseUrl: "https://api.moonshot.ai/v1",
    model: "kimi-k3.1-pro",
    models: ["kimi-k3.1-pro", "kimi-k3.1-pro-highspeed", "kimi-k3", "kimi-k2.6"],
  },
  {
    id: "minimax",
    label: "MiniMax",
    provider: "openAiCompatible",
    baseUrl: "https://api.minimax.io/v1",
    model: "MiniMax-M3.1-Flash",
    models: ["MiniMax-M3.1-Flash", "MiniMax-M3"],
  },
  {
    id: "ollama",
    label: "Ollama (local)",
    provider: "openAiCompatible",
    baseUrl: "http://localhost:11434/v1",
    model: "llama3.2",
    models: [],
    embeddingModel: "nomic-embed-text",
    hint: "no API key needed; start `ollama serve` and pull a model (and an embedding model for SEMANTIC)",
  },
  {
    id: "lmstudio",
    label: "LM Studio (local)",
    provider: "openAiCompatible",
    baseUrl: "http://localhost:1234/v1",
    model: "local-model",
    models: [],
    hint: "start the LM Studio local server; leave the API key blank",
  },
  {
    id: "custom",
    label: "Custom / other local",
    provider: "openAiCompatible",
    baseUrl: "http://localhost:11434/v1",
    model: "llama3.2",
    models: [],
    hint: "any OpenAI-compatible endpoint; leave the key blank for local servers",
  },
];

export const AI_DEFAULTS: AiConfig = {
  enabled: false,
  preset: "openrouter",
  provider: "openAiCompatible",
  baseUrl: "https://openrouter.ai/api/v1",
  model: "deepseek/deepseek-v4.1-flash-1017",
  embeddingModel: "",
  apiKey: "",
  timeoutSecs: 30,
  discogsToken: "",
};

export function presetById(id: string): AiPreset | undefined {
  return AI_PRESETS.find((p) => p.id === id);
}

/** Persisted settings; missing fields fall back to defaults on the backend. */
export function aiGetConfig(): Promise<AiConfig> {
  return invoke<AiConfig>("ai_get_config");
}

/** Persist settings; resolves with the sanitized values actually stored. */
export function aiSetConfig(config: AiConfig): Promise<AiConfig> {
  return invoke<AiConfig>("ai_set_config", { config });
}

/** Tiny completion against a draft config (does not save it). */
export function aiTest(config: AiConfig): Promise<string> {
  return invoke<string>("ai_test", { config });
}

/**
 * Fetch the provider's model list (`/models`; `/v1/models` for Anthropic).
 * Used by the settings picker so it does not go stale.
 */
export function aiModels(config: AiConfig): Promise<string[]> {
  return invoke<string[]>("ai_models", { config });
}

export interface AiSearchReport {
  def: SmartViewDef | null;
  explanation: string;
  catalog: DiscogsRelease[];
  /** Total releases Discogs reported across the fetched pages (0 = unknown). */
  catalogTotal: number;
  preset: string;
  model: string;
  toolCalls: number;
}

/** One Discogs catalog entry (shown even when no local tracks match). */
export interface DiscogsRelease {
  id: number;
  title: string;
  year: string | null;
  label: string[];
  format: string[];
  genre: string[];
  style: string[];
  country: string;
  kind: string;
}

/**
 * Translate a prompt (optionally refining `baseDef`) into a validated Smart
 * View definition. The caller runs `smartViewQuery(report.def)` for rows.
 */
export function aiSearch(
  prompt: string,
  baseDef: SmartViewDef | null = null,
): Promise<AiSearchReport> {
  return invoke<AiSearchReport>("ai_search", { prompt, baseDef });
}

/**
 * Deterministic offline rules search — never contacts a provider. Used by the
 * onboarding AI demo so it works with zero configuration and no network.
 */
export function aiOfflineSearch(prompt: string): Promise<AiSearchReport> {
  return invoke<AiSearchReport>("ai_offline_search", { prompt });
}

// ── semantic search (EMBEDDINGS) ──────────────────────────────────────────

/** Progress event while a semantic index build is running. */
export const SEMANTIC_PROGRESS_EVENT = "semantic-progress";

export interface SemanticProgress {
  model: string;
  done: number;
  total: number;
}

/** Index coverage and setup state for SEMANTIC search. */
export interface SemanticStatus {
  configured: boolean;
  providerReady: boolean;
  model: string;
  total: number;
  indexed: number;
  stale: number;
  /** Actionable setup hint when `configured` is false. */
  message: string | null;
}

export interface SemanticIndexReport {
  model: string;
  embedded: number;
  total: number;
}

export interface SemanticSearchReport {
  tracks: Track[];
  model: string;
  indexed: number;
  dims: number;
  explanation: string;
}

/** Current embedding coverage for the configured model. */
export function semanticStatus(): Promise<SemanticStatus> {
  return invoke<SemanticStatus>("semantic_status");
}

/**
 * Embed missing/changed tracks. `rebuild` drops the model's vectors first and
 * re-embeds the whole library. Progress arrives via SEMANTIC_PROGRESS_EVENT.
 */
export function semanticIndex(rebuild = false): Promise<SemanticIndexReport> {
  return invoke<SemanticIndexReport>("semantic_index", { rebuild });
}

/** Embed the prompt and return the nearest tracks by cosine similarity. */
export function semanticSearch(
  prompt: string,
  limit = 100,
): Promise<SemanticSearchReport> {
  return invoke<SemanticSearchReport>("semantic_search", { prompt, limit });
}

/** Broadcast after the onboarding wizard saves ai.json (all windows). */
export const AI_CONFIG_EVENT = "ai-config-changed";

export function notifyAiConfigChanged(): Promise<void> {
  return emit(AI_CONFIG_EVENT);
}
