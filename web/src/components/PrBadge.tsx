// Status of a linked GitHub PR: a compact badge for cards and a row of chips
// for the detail view. Without a known status (no GitHub token on the server,
// offline, no GitHub PR link) the card shows the plain "PR" link as before.

import { Show } from "solid-js";
import { PR_CHECKS_LABELS, PR_REVIEW_LABELS, PR_STATE_LABELS } from "../labels";
import { prStatus } from "../state/prs";
import type { PrChecks, PrReview, PrStatus } from "../sync/types";
import { Icon, type IconName } from "./Icon";

const REVIEW_ICONS: Record<PrReview, IconName> = {
  requested: "eye",
  approved: "check",
  changes_requested: "pencil",
};

const CHECKS_ICONS: Record<PrChecks, IconName> = {
  pending: "circle",
  success: "checkCircle",
  failure: "x",
};

/** One line for tooltips: "#42 Titel · Offen · Freigegeben · CI rot". */
export function prSummary(s: PrStatus): string {
  const parts = [`#${s.number} ${s.title}`, PR_STATE_LABELS[s.state]];
  if (s.review) parts.push(PR_REVIEW_LABELS[s.review]);
  if (s.checks) parts.push(PR_CHECKS_LABELS[s.checks]);
  return parts.join(" · ");
}

/** The card's PR link with state, review and CI (falls back to "PR"). */
export function PrBadge(props: { url: string }) {
  const status = () => prStatus(props.url);
  return (
    <a
      class="card-pr pr-badge"
      data-state={status()?.state}
      href={props.url}
      target="_blank"
      rel="noopener noreferrer"
      title={status() ? prSummary(status()!) : "PR öffnen"}
      onClick={(e) => e.stopPropagation()}
    >
      <Icon name="pullRequest" size={12} />
      <Show when={status()} fallback="PR">
        {(s) => (
          <>
            <span class="pr-badge-state">
              #{s().number} {PR_STATE_LABELS[s().state]}
            </span>
            <Show when={s().review}>
              {(r) => (
                <span class="pr-mark" data-review={r()} aria-label={PR_REVIEW_LABELS[r()]}>
                  <Icon name={REVIEW_ICONS[r()]} size={11} />
                </span>
              )}
            </Show>
            <Show when={s().checks}>
              {(c) => (
                <span class="pr-mark" data-checks={c()} aria-label={PR_CHECKS_LABELS[c()]}>
                  <Icon name={CHECKS_ICONS[c()]} size={11} />
                </span>
              )}
            </Show>
          </>
        )}
      </Show>
    </a>
  );
}

/** Detail view: state, review and CI as labelled chips plus the PR title. */
export function PrStatusChips(props: { url: string }) {
  const status = () => prStatus(props.url);
  return (
    <Show when={status()}>
      {(s) => (
        <div class="pr-status" data-testid="pr-status">
          <span class="pr-chip" data-state={s().state}>
            <Icon name="pullRequest" size={12} />
            {PR_STATE_LABELS[s().state]}
          </span>
          <Show when={s().review}>
            {(r) => (
              <span class="pr-chip" data-review={r()}>
                <Icon name={REVIEW_ICONS[r()]} size={12} />
                {PR_REVIEW_LABELS[r()]}
              </span>
            )}
          </Show>
          <Show when={s().checks}>
            {(c) => (
              <span class="pr-chip" data-checks={c()}>
                <Icon name={CHECKS_ICONS[c()]} size={12} />
                {PR_CHECKS_LABELS[c()]}
              </span>
            )}
          </Show>
          <span class="pr-title" title={s().title}>
            #{s().number} {s().title}
          </span>
        </div>
      )}
    </Show>
  );
}
