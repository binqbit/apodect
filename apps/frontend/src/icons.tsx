import { useState } from "react";
import type { Chain } from "./api";

const paths = {
  check: "m5 12 4 4L19 6",
  confirmed: "M22 11.1V12a10 10 0 1 1-5.9-9.1M22 4 12 14l-3-3",
  clock: "M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0ZM12 6v6l4 2",
  pause: "M8 5v14M16 5v14",
  play: "m8 4 12 8-12 8V4Z",
  stop: "M5 5h14v14H5Z",
  canceled: "M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0ZM8 8l8 8M16 8l-8 8",
  alert: "M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0ZM12 7v5M12 16h.01",
  info: "M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0ZM12 11v6M12 7h.01",
  queue: "M9 6h11M9 12h11M9 18h11M3 6h.01M3 12h.01M3 18h.01",
  transfer: "M4 7h16m-4-4 4 4-4 4M20 17H4m4-4-4 4 4 4",
  wallet:
    "M20 8V5a2 2 0 0 0-2-2H5a3 3 0 0 0 0 6h16v11H5a3 3 0 0 1-3-3V6M21 13h-5v3h5",
  copy: "M9 9h12v12H9ZM15 5V3H3v12h2",
  projects: "M3 3h7v7H3ZM14 3h7v7h-7ZM3 14h7v7H3ZM14 14h7v7h-7Z",
  user: "M16 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0ZM4 21v-2a6 6 0 0 1 6-6h4a6 6 0 0 1 6 6v2",
  logout: "M9 3H3v18h6M9 12h12m-5-5 5 5-5 5",
  book: "M12 5v16M12 5C8 2 4 3 2 4v16c3-2 7-1 10 1 3-2 7-3 10-1V4c-2-1-6-2-10 1Z",
  shield: "m12 2 9 4v6c0 5-9 10-9 10S3 17 3 12V6l9-4Zm-5 9 3 3 7-7",
  manager: "M9 6V3h6v3M3 6h18v14H3ZM3 11c6 4 12 4 18 0M12 11v4",
  link: "m10 13 4-4M8 16l-1 1a4 4 0 0 1-6-6l5-5a4 4 0 0 1 6 0M16 8l1-1a4 4 0 0 1 6 6l-5 5a4 4 0 0 1-6 0",
  plus: "M12 5v14M5 12h14",
  refresh: "M20 7V2m0 5h-5M4 17v5m0-5h5M20 7A9 9 0 0 0 4 6M4 17a9 9 0 0 0 16 1",
  arrow: "M5 12h14m-6-6 6 6-6 6",
} as const;

export type IconName = keyof typeof paths;

/** Decorative icons always accompany a visible label or an accessible button name. */
export function Icon({
  name,
  className = "",
}: {
  name: IconName;
  className?: string;
}) {
  return (
    <svg
      className={`icon ${className}`}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      <path d={paths[name]} />
    </svg>
  );
}

export function NetworkIcon({ chain }: { chain: Chain }) {
  return (
    <svg
      className={`network-mark network-mark-${chain}`}
      viewBox="0 0 24 24"
      aria-hidden="true"
      focusable="false"
    >
      {chain === "ethereum" ? (
        <>
          <path fill="currentColor" d="m12 1 7 11-7 4-7-4Zm0 17 7-4-7 9-7-9Z" />
          <path
            fill="var(--paper)"
            opacity=".3"
            d="M12 1v15l7-4Zm0 17v5l7-9Z"
          />
        </>
      ) : chain === "solana" ? (
        <>
          <path fill="#a89aff" d="M6 4h16l-4 4H2Z" />
          <path fill="#81cfc6" d="M2 10h16l4 4H6Z" />
          <path fill="#a89aff" d="M6 16h16l-4 4H2Z" />
        </>
      ) : (
        <path
          fill="currentColor"
          d="M5.2 3a2.2 2.2 0 0 0-2.2 2.2v13.6a2.2 2.2 0 0 0 3.9 1.4L10 16.5l-1.3-1.1-3.2 3.7V5.8l12 14.4a2.2 2.2 0 0 0 3.9-1.4V5.2a2.2 2.2 0 0 0-3.9-1.4l-3.1 3.7 1.3 1.1 3.2-3.7v13.3L6.9 3.8A2.2 2.2 0 0 0 5.2 3Z"
        />
      )}
    </svg>
  );
}

export function WalletIcon({ src }: { src?: string }) {
  const [failedSource, setFailedSource] = useState<string>();
  // Wallet discovery supplies data images; bundled wallet modules use local assets.
  // Render SVGs as images, never as provider-supplied markup.
  const supported =
    src &&
    (/^data:image\/(png|webp|jpeg|gif|svg\+xml)[;,]/i.test(src) ||
      /^\/(?!\/)/.test(src));
  return (
    <span className="wallet-provider-icon" aria-hidden="true">
      {supported && src !== failedSource ? (
        <img src={src} alt="" onError={() => setFailedSource(src)} />
      ) : (
        <Icon name="wallet" />
      )}
    </span>
  );
}
