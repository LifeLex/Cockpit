interface EmptyStateProps {
  readonly icon: string;
  readonly title: string;
  readonly description: string;
  readonly actionLabel?: string | undefined;
  readonly onAction?: (() => void) | undefined;
}

/** Reusable empty state with icon, title, description, and optional action button. */
export function EmptyState({
  icon,
  title,
  description,
  actionLabel,
  onAction,
}: EmptyStateProps) {
  return (
    <div className="flex flex-col items-center justify-center rounded-lg border border-border bg-surface-1 px-8 py-12 text-center">
      <span className="mb-4 text-4xl">{icon}</span>
      <h3 className="mb-2 text-base font-semibold text-text-primary">{title}</h3>
      <p className="mb-4 max-w-sm text-sm text-text-secondary">
        {description}
      </p>
      {actionLabel !== undefined && onAction !== undefined && (
        <button
          onClick={onAction}
          className="rounded-md bg-accent px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-accent-hover"
        >
          {actionLabel}
        </button>
      )}
    </div>
  );
}
