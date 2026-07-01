import type { LucideIcon } from "lucide-react";

import { Button } from "@/components/ui/button";

interface EmptyStateProps {
  /** Lucide icon component rendered inside a tokened tile. */
  readonly icon: LucideIcon;
  readonly title: string;
  readonly description: string;
  readonly actionLabel?: string | undefined;
  readonly onAction?: (() => void) | undefined;
}

export function EmptyState({
  icon: Icon,
  title,
  description,
  actionLabel,
  onAction,
}: EmptyStateProps) {
  return (
    <div className="flex flex-col items-center justify-center rounded-xl border border-border/50 bg-card px-8 py-12 text-center">
      <span
        className="flex h-14 w-14 items-center justify-center rounded-xl border border-border/60 bg-muted/40 text-muted-foreground"
        aria-hidden="true"
      >
        <Icon className="h-6 w-6" strokeWidth={1.75} />
      </span>
      <h3 className="mt-4 text-base font-semibold text-foreground">
        {title}
      </h3>
      <p className="mt-1.5 max-w-sm text-sm text-muted-foreground">
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
