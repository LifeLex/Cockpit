import { Button } from "@/components/ui/button";

interface EmptyStateProps {
  readonly icon: string;
  readonly title: string;
  readonly description: string;
  readonly actionLabel?: string | undefined;
  readonly onAction?: (() => void) | undefined;
}

export function EmptyState({
  icon,
  title,
  description,
  actionLabel,
  onAction,
}: EmptyStateProps) {
  return (
    <div className="flex flex-col items-center justify-center rounded-xl border border-border/50 bg-surface-1 px-8 py-12 text-center">
      <span className="text-5xl" role="img" aria-label={title}>
        {icon}
      </span>
      <h3 className="mt-4 text-base font-semibold text-text-primary">
        {title}
      </h3>
      <p className="mt-1.5 max-w-sm text-sm text-text-muted">
        {description}
      </p>
      {actionLabel !== undefined && onAction !== undefined && (
        <Button
          variant="outline"
          className="mt-5"
          onClick={onAction}
        >
          {actionLabel}
        </Button>
      )}
    </div>
  );
}
