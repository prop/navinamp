/**
 * AI search IPC + provider presets.
 *
 * Keys live in `<app_config>/ai.json` (plaintext, like `navidrome.json`) and
 * never leave the Rust side except in the settings editor itself. All
 * providers except Anthropic speak the OpenAI `/chat/completions` shape;
 * Anthropic gets its own adapter in `navinamp-llm`.
 */

import { invoke } from "@tauri-apps/api/core";
import type { SmartViewDef } from "../library/ipc";

export type AiProviderKind = "openAiCompatible" | "anthropic";

export interface AiConfig {
  enabled: boolean;
  /** Preset id (`openrouter`, `openai`, `anthropic`, `custom`, …). */
  preset: string;
  provider: AiProviderKind;
  baseUrl: string;
  model: string;
  apiKey: string;
  timeoutSecs: number;
  /** Optional Discogs personal access token (higher rate limit). */
  discogsToken: string;
}

export interface AiPreset {
  id: string;
  label: string;
  provider: AiProviderKind;
  baseUrl: string;
  model: string;
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
    model: "openai/gpt-4o-mini",
    hint: "one key for OpenAI, Claude, Gemini, DeepSeek, GLM, Kimi, MiniMax…",
  },
  {
    id: "openai",
    label: "OpenAI",
    provider: "openAiCompatible",
    baseUrl: "https://api.openai.com/v1",
    model: "gpt-4o-mini",
  },
  {
    id: "anthropic",
    label: "Anthropic (Claude)",
    provider: "anthropic",
    baseUrl: "https://api.anthropic.com",
    model: "claude-sonnet-4-5",
  },
  {
    id: "deepseek",
    label: "DeepSeek",
    provider: "openAiCompatible",
    baseUrl: "https://api.deepseek.com/v1",
    model: "deepseek-chat",
  },
  {
    id: "glm",
    label: "Zhipu GLM",
    provider: "openAiCompatible",
    baseUrl: "https://open.bigmodel.cn/api/paas/v4",
    model: "glm-4-flash",
  },
  {
    id: "kimi",
    label: "Moonshot Kimi",
    provider: "openAiCompatible",
    baseUrl: "https://api.moonshot.cn/v1",
    model: "moonshot-v1-8k",
  },
  {
    id: "minimax",
    label: "MiniMax",
    provider: "openAiCompatible",
    baseUrl: "https://api.minimax.chat/v1",
    model: "MiniMax-Text-01",
  },
  {
    id: "custom",
    label: "Custom / local (Ollama, LM Studio…)",
    provider: "openAiCompatible",
    baseUrl: "http://localhost:11434/v1",
    model: "llama3.2",
    hint: "any OpenAI-compatible endpoint; leave the key blank for local servers",
  },
];

export const AI_DEFAULTS: AiConfig = {
  enabled: false,
  preset: "openrouter",
  provider: "openAiCompatible",
  baseUrl: "https://openrouter.ai/api/v1",
  model: "openai/gpt-4o-mini",
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

export interface AiSearchReport {
  def: SmartViewDef;
  explanation: string;
  preset: string;
  model: string;
  toolCalls: number;
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
