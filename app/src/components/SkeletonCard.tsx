/** Skeleton placeholder that mimics the shape of a ReviewCard while loading. */
export function SkeletonCard() {
  return (
    <div className="mb-3 rounded-lg border border-border p-4 animate-pulse">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <div className="h-4 w-20 rounded bg-surface-2" />
          <div className="h-4 w-32 rounded bg-surface-2" />
        </div>
        <div className="h-5 w-16 rounded bg-surface-2" />
      </div>
      <div className="mt-3 flex items-center gap-4">
        <div className="h-3 w-24 rounded bg-surface-2" />
        <div className="h-3 w-16 rounded bg-surface-2" />
      </div>
    </div>
  );
}

/** Renders a list of skeleton cards as a loading placeholder. */
export function SkeletonList({ count = 5 }: { readonly count?: number }) {
  return (
    <>
      {Array.from({ length: count }, (_, i) => (
        <SkeletonCard key={i} />
      ))}
    </>
  );
}
