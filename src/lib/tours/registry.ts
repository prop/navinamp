/**
 * Onboarding tour definitions.
 *
 * Tours are declarative and versioned: `since` is the first app version that
 * ships the feature, and update prompts only offer tours newer than the user's
 * last-seen version. Steps can spotlight a target in the library window or
 * render an interactive panel (the AI demo / provider wizard).
 */

/** Interactive content rendered inside a tour step instead of plain text. */
export type TourPanel = "aiDemo" | "aiWizard";

export interface TourStep {
  title: string;
  /** Short card copy. Panel steps usually keep this to one sentence. */
  body?: string;
  /** CSS selector in the library window; omitted = centered card. */
  target?: string;
  panel?: TourPanel;
}

export interface Tour {
  id: string;
  /** First app version that contains this tour (semver). */
  since: string;
  title: string;
  steps: TourStep[];
}

const AI_DEMO_STEP: TourStep = {
  title: "AI search — no key needed",
  body: "The offline parser turns a phrase into library rules locally. Try the demo; no account or network call is involved.",
  panel: "aiDemo",
};

const AI_WIZARD_STEP: TourStep = {
  title: "Connect a provider (optional)",
  body: "Full free-form search and Discogs catalog lookups use a provider. OpenRouter is the easiest single key; a local Ollama/LM Studio endpoint also works.",
  panel: "aiWizard",
};

const WELCOME_TOUR: Tour = {
  id: "welcome.v1",
  since: "0.0.1",
  title: "Welcome tour",
  steps: [
    {
      title: "Welcome to NaVinaMP",
      body: "Your player (drag its titlebar; double-click to shade) and playlist queue are already open. The Media Library adds scans, Navidrome, smart views and search.",
    },
    {
      title: "Four ways to search",
      body: "SIMPLE searches text, ADVANCED builds field rules, AI turns a sentence into a rule set, and SEMANTIC finds tracks by meaning with embeddings.",
      target: '[data-tour="search"]',
    },
    AI_DEMO_STEP,
    AI_WIZARD_STEP,
    {
      title: "You're set",
      body: "Replay this tour from the TOUR button in this header, or open AI settings any time. Enjoy the music.",
    },
  ],
};

const AI_SEARCH_TOUR: Tour = {
  id: "ai-search.v1",
  since: "0.10.0",
  title: "AI search",
  steps: [
    {
      title: "Describe the music",
      body: "AI mode accepts a sentence and builds a Smart View. It can also list releases from Discogs for label and discography questions.",
      target: '[data-tour="ai-mode"]',
    },
    AI_DEMO_STEP,
    AI_WIZARD_STEP,
    {
      title: "That's AI search",
      body: "Your generated rules stay editable in ADVANCED, and results can be saved as a smart view or a playlist.",
    },
  ],
};

export const TOURS: Tour[] = [WELCOME_TOUR, AI_SEARCH_TOUR];

export function tourById(id: string): Tour | undefined {
  return TOURS.find((tour) => tour.id === id);
}
