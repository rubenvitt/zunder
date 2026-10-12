// The Zunder mark: an ember flame on a dark tile (same artwork as the app icon).

export function Logo(props: { size?: number }) {
  const size = () => props.size ?? 24;
  return (
    <svg class="logo-mark" width={size()} height={size()} viewBox="0 0 64 64" aria-hidden="true">
      <defs>
        <linearGradient id="zl-tile" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stop-color="#2b211c" />
          <stop offset="1" stop-color="#120e0c" />
        </linearGradient>
        <linearGradient id="zl-outer" x1="0" y1="1" x2="0" y2="0">
          <stop offset="0" stop-color="#ff4d1f" />
          <stop offset="1" stop-color="#ffa63d" />
        </linearGradient>
        <linearGradient id="zl-inner" x1="0" y1="1" x2="0" y2="0">
          <stop offset="0" stop-color="#ffc35c" />
          <stop offset="1" stop-color="#fff1b8" />
        </linearGradient>
      </defs>
      <rect width="64" height="64" rx="15" fill="url(#zl-tile)" />
      <path fill="url(#zl-outer)" d="M33.5 8.5c1.6 7.4 11.9 12.6 11.9 26.1 0 9.6-6 17.4-13.4 17.4s-13.4-7.1-13.4-15.7c0-6.4 2.9-10.9 6.6-14 .1 4.6 2 7.4 4.6 8.7-.9-9.3.7-16.9 3.7-22.5Z" />
      <path fill="url(#zl-inner)" d="M32.6 30.6c2.1 4.7 7 7.6 7 13.6 0 4.5-3.4 7.8-7.6 7.8s-7.6-3.3-7.6-7.6c0-3.2 1.7-5.3 3.8-6.6.4 2.6 1.4 3.9 2.8 4.5-.5-5.1.2-8.5 1.6-11.7Z" />
    </svg>
  );
}
