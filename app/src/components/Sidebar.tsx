/**
 * Left sidebar navigation for the Cockpit app.
 *
 * Renders the logo, primary navigation items (Reviews, Plan, Stacks),
 * and secondary actions (Kickoff, Settings) separated by a divider.
 * Active state uses a left accent border and `bg-surface-2` highlight.
 */

import type { ViewState } from "../store";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** The subset of ViewState kinds that correspond to sidebar nav items. */
type NavKind = ViewState["kind"];

interface SidebarProps {
  /** The currently active view kind. */
  readonly activeView: NavKind;
  /** Number of reviews to display as a badge. */
  readonly reviewCount: number;
  /** Whether a plan is currently loaded. */
  readonly hasPlan: boolean;
  /** Callback to navigate to a different view. */
  readonly onNavigate: (kind: NavKind) => void;
}

// ---------------------------------------------------------------------------
// Icons (inline SVG to avoid external dependencies)
// ---------------------------------------------------------------------------

/** Simple list/reviews icon. */
function ReviewsIcon() {
  return (
    <svg
      className="h-4 w-4 shrink-0"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <rect x="2" y="2" width="12" height="12" rx="2" />
      <path d="M5 6h6M5 8.5h6M5 11h3" />
    </svg>
  );
}

/** Plan/document icon. */
function PlanIcon() {
  return (
    <svg
      className="h-4 w-4 shrink-0"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M9 2H4a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1V6L9 2z" />
      <path d="M9 2v4h4" />
    </svg>
  );
}

/** Stack/layers icon. */
function StacksIcon() {
  return (
    <svg
      className="h-4 w-4 shrink-0"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M8 2 2 5l6 3 6-3-6-3z" />
      <path d="m2 8 6 3 6-3" />
      <path d="m2 11 6 3 6-3" />
    </svg>
  );
}

/** Rocket/kickoff icon. */
function KickoffIcon() {
  return (
    <svg
      className="h-4 w-4 shrink-0"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M8 1s4 2 4 7-4 7-4 7-4-2-4-7 4-7 4-7z" />
      <circle cx="8" cy="7" r="1.5" />
      <path d="M5 12 3 15M11 12l2 3" />
    </svg>
  );
}

/** Gear/settings icon. */
function SettingsIcon() {
  return (
    <svg
      className="h-4 w-4 shrink-0"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <circle cx="8" cy="8" r="2" />
      <path d="M13.3 9.7a1.2 1.2 0 0 0 .2 1.3l.1.1a1.5 1.5 0 1 1-2.1 2.1l-.1-.1a1.2 1.2 0 0 0-1.3-.2 1.2 1.2 0 0 0-.7 1.1v.2a1.5 1.5 0 0 1-3 0V14a1.2 1.2 0 0 0-.8-1.1 1.2 1.2 0 0 0-1.3.2l-.1.1a1.5 1.5 0 1 1-2.1-2.1l.1-.1a1.2 1.2 0 0 0 .2-1.3 1.2 1.2 0 0 0-1.1-.7H2a1.5 1.5 0 0 1 0-3h.2A1.2 1.2 0 0 0 3.2 5.7a1.2 1.2 0 0 0-.2-1.3l-.1-.1a1.5 1.5 0 1 1 2.1-2.1l.1.1a1.2 1.2 0 0 0 1.3.2h.1A1.2 1.2 0 0 0 7 1.4V1.2a1.5 1.5 0 0 1 3 0v.2a1.2 1.2 0 0 0 .7 1.1 1.2 1.2 0 0 0 1.3-.2l.1-.1a1.5 1.5 0 1 1 2.1 2.1l-.1.1a1.2 1.2 0 0 0-.2 1.3v.1a1.2 1.2 0 0 0 1.1.7h.2a1.5 1.5 0 0 1 0 3h-.2a1.2 1.2 0 0 0-1.1.8z" />
    </svg>
  );
}

// ---------------------------------------------------------------------------
// NavItem
// ---------------------------------------------------------------------------

interface NavItemProps {
  readonly label: string;
  readonly icon: React.ReactNode;
  readonly active: boolean;
  readonly badge?: string | undefined;
  readonly onClick: () => void;
}

function NavItem({ label, icon, active, badge, onClick }: NavItemProps) {
  return (
    <button
      onClick={onClick}
      className={[
        "flex w-full items-center gap-3 rounded-md px-3 py-2 text-sm font-medium transition-colors",
        active
          ? "border-l-2 border-accent bg-surface-2 text-text-primary"
          : "border-l-2 border-transparent text-text-secondary hover:bg-surface-2 hover:text-text-primary",
      ].join(" ")}
    >
      {icon}
      <span className="flex-1 text-left">{label}</span>
      {badge !== undefined && (
        <span className="rounded-full bg-surface-3 px-2 py-0.5 text-xs text-text-muted">
          {badge}
        </span>
      )}
    </button>
  );
}

// ---------------------------------------------------------------------------
// Sidebar
// ---------------------------------------------------------------------------

export function Sidebar({
  activeView,
  reviewCount,
  hasPlan,
  onNavigate,
}: SidebarProps) {
  return (
    <aside className="flex w-60 shrink-0 flex-col border-r border-border bg-surface-1">
      {/* Logo / app name */}
      <div className="flex items-center gap-2 px-4 py-5">
        <div className="flex h-7 w-7 items-center justify-center rounded-md bg-accent text-sm font-bold text-white">
          C
        </div>
        <span className="text-base font-semibold text-text-primary">
          Cockpit
        </span>
      </div>

      {/* Primary navigation */}
      <nav className="flex flex-1 flex-col gap-1 px-3">
        <NavItem
          label="Reviews"
          icon={<ReviewsIcon />}
          active={activeView === "frontier" || activeView === "diff"}
          badge={reviewCount > 0 ? String(reviewCount) : undefined}
          onClick={() => {
            onNavigate("frontier");
          }}
        />
        <NavItem
          label="Plan"
          icon={<PlanIcon />}
          active={activeView === "plan"}
          badge={hasPlan ? "loaded" : undefined}
          onClick={() => {
            onNavigate("plan");
          }}
        />
        <NavItem
          label="Stacks"
          icon={<StacksIcon />}
          active={activeView === "stacks"}
          onClick={() => {
            onNavigate("stacks");
          }}
        />

        {/* Spacer pushes secondary nav to the bottom */}
        <div className="flex-1" />

        {/* Divider */}
        <div className="mx-1 border-t border-border" />

        {/* Secondary navigation */}
        <div className="flex flex-col gap-1 py-2">
          <NavItem
            label="Kickoff"
            icon={<KickoffIcon />}
            active={activeView === "kickoff"}
            onClick={() => {
              onNavigate("kickoff");
            }}
          />
          <NavItem
            label="Settings"
            icon={<SettingsIcon />}
            active={activeView === "settings"}
            onClick={() => {
              onNavigate("settings");
            }}
          />
        </div>
      </nav>
    </aside>
  );
}
