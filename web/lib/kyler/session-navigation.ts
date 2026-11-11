export type NavigationMode = "push" | "replace" | "none";

export function sessionFromUrl(href: string): string | null {
  const match = new URL(href).pathname.match(/^\/chat\/([^/]+)\/?$/);
  if (!match) return null;
  try {
    return decodeURIComponent(match[1]) || null;
  } catch {
    return null;
  }
}

export function sessionUrl(href: string, session: string | null): string {
  const url = new URL(href);
  url.pathname = session ? `/chat/${encodeURIComponent(session)}` : "/";
  url.searchParams.delete("session");
  return `${url.pathname}${url.search}${url.hash}`;
}

export function navigateSession(session: string | null, mode: NavigationMode): void {
  if (mode === "none") return;
  const url = sessionUrl(window.location.href, session);
  if (url === `${window.location.pathname}${window.location.search}${window.location.hash}`) return;
  // Next.js integrates native history updates without remounting active conversations.
  if (mode === "replace") window.history.replaceState(null, "", url);
  else window.history.pushState(null, "", url);
}
