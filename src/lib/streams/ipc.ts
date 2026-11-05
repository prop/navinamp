/**
 * Typed IPC for playlist URL streams (Add URL) and the optional YouTube
 * helper (`yt-dlp`).
 *
 * Direct `http(s)` audio URLs are played by the audio engine as remote
 * sources; this module only classifies them and derives display metadata.
 * YouTube links need a per-play resolver because direct media URLs expire.
 */

import { invoke } from "@tauri-apps/api/core";

/** Current state of the user-provided yt-dlp binary. */
export interface YtdlpStatus {
  /** Saved override path; null means auto-detect. */
  configuredPath: string | null;
  /** Executable that will run; null when not found. */
  resolvedPath: string | null;
  found: boolean;
  /** `yt-dlp --version` first line, when available. */
  version: string | null;
}

/** A validated playlist URL plus display metadata. */
export interface ParsedStreamUrl {
  /** Normalized URL to store in the queue. */
  url: string;
  /** True for a single-video YouTube link (needs yt-dlp). */
  youtube: boolean;
  title: string;
  artist: string;
}

/** One YouTube resolution; `streamUrl` is the expiring direct media URL. */
export interface ResolvedStream {
  streamUrl: string;
  title: string;
  artist: string;
  durationSecs: number | null;
}

/** Validate a URL and derive host/title metadata. */
export function streamsParseUrl(url: string): Promise<ParsedStreamUrl> {
  return invoke<ParsedStreamUrl>("streams_parse_url", { url });
}

/** Discover the configured/PATH/common-location yt-dlp. */
export function streamsYtdlpStatus(): Promise<YtdlpStatus> {
  return invoke<YtdlpStatus>("streams_ytdlp_status");
}

/** Save a custom yt-dlp path; `null` clears it and re-enables auto-detect. */
export function streamsSetYtdlpPath(path: string | null): Promise<YtdlpStatus> {
  return invoke<YtdlpStatus>("streams_set_ytdlp_path", { path });
}

/** Resolve a YouTube link to a direct audio URL + metadata (re-run per play). */
export function streamsResolve(url: string): Promise<ResolvedStream> {
  return invoke<ResolvedStream>("streams_resolve", { url });
}
