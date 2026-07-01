import { useId } from "react";
import { cn } from "@/lib/utils";

interface LogoProps {
  readonly size?: number | undefined;
  readonly className?: string | undefined;
}

/**
 * Cockpit brand mark — the attitude-indicator "horizon" (matches the app icon).
 *
 * A rounded-square instrument: teal sky over a dark ground with a slight bank,
 * level aircraft wings, and an amber bank pointer. Uses fixed instrument colors
 * (a brand badge, theme-independent) like the app icon.
 */
export function Logo({ size = 28, className }: LogoProps) {
  const clip = useId();
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 32 32"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      className={cn("shrink-0", className)}
      aria-hidden="true"
    >
      <defs>
        <clipPath id={clip}>
          <rect x="1" y="1" width="30" height="30" rx="8" />
        </clipPath>
      </defs>
      <g clipPath={`url(#${clip})`}>
        {/* banked sky / ground */}
        <g transform="rotate(-9 16 16)">
          <rect x="-14" y="-14" width="60" height="30.5" fill="#4fd4d6" />
          <rect x="-14" y="16.5" width="60" height="40" fill="#141b22" />
          <rect x="-14" y="15.9" width="60" height="1.1" fill="#0b0e12" />
        </g>
        {/* level aircraft reference */}
        <g stroke="#f2f6fa" strokeWidth="2.3" strokeLinecap="round">
          <path d="M6.5 16 H12.5" />
          <path d="M19.5 16 H25.5" />
        </g>
        <circle cx="16" cy="16" r="1.25" fill="#f2f6fa" />
        {/* amber bank pointer */}
        <path d="M16 5.4 l1.4 -2.2 h-2.8 z" fill="#f2b544" />
      </g>
    </svg>
  );
}
