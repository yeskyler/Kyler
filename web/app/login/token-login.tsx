"use client";

import { useState, type FormEvent } from "react";
import {
  AlertCircleIcon,
  ArrowRightIcon,
  EyeIcon,
  EyeOffIcon,
  KeyRoundIcon,
  LoaderCircleIcon,
  TerminalIcon,
} from "lucide-react";
import { BrandMark } from "@/components/kyler/brand-mark";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

export function TokenLogin({ returnTo }: { returnTo: string }) {
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [showToken, setShowToken] = useState(false);
  const [error, setError] = useState("");
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const response = await fetch("/api/auth/login", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ token }),
        cache: "no-store",
      });
      if (!response.ok) {
        const body = await response.json();
        throw new Error(body.message ?? "Sign-in failed.");
      }
      setToken("");
      window.location.replace(returnTo);
    } catch (error) {
      setError(error instanceof Error ? error.message : "Cannot sign in. Try again.");
      setBusy(false);
    }
  }
  return (
    <main className="flex min-h-dvh flex-col bg-muted/40 px-5 py-6 sm:px-8 sm:py-8">
      <header className="mx-auto flex w-full max-w-6xl items-center gap-2.5" aria-label="Kyler">
        <BrandMark size={30} />
        <span className="text-lg font-semibold tracking-tight">Kyler</span>
      </header>
      <div className="flex flex-1 items-center justify-center py-10 sm:py-14">
        <section
          className="w-full max-w-[440px] rounded-3xl border border-border/80 bg-card p-6 shadow-[0_12px_48px_-24px_rgb(0_0_0/0.2)] sm:p-9"
          aria-labelledby="login-heading"
        >
          <div className="mb-8">
            <p className="mb-3 text-[11px] font-medium tracking-[0.16em] text-muted-foreground uppercase">
              Workspace access
            </p>
            <h1
              id="login-heading"
              className="text-[26px] leading-tight font-semibold tracking-tight sm:text-[28px]"
            >
              Welcome to Kyler
            </h1>
            <p className="mt-3 text-sm leading-6 text-muted-foreground">
              Enter your Web token to open your workspace.
            </p>
          </div>
          <form onSubmit={submit} aria-busy={busy}>
            <label htmlFor="web-token" className="mb-2.5 block text-sm font-medium">
              Web token
            </label>
            <div className="relative">
              <KeyRoundIcon
                className="pointer-events-none absolute top-1/2 left-3.5 z-10 size-4 -translate-y-1/2 text-muted-foreground"
                aria-hidden
              />
              <Input
                id="web-token"
                name="token"
                type={showToken ? "text" : "password"}
                autoComplete="off"
                autoCapitalize="none"
                spellCheck={false}
                maxLength={256}
                required
                autoFocus
                disabled={busy}
                placeholder="Paste your Web token"
                value={token}
                onChange={(event) => {
                  setToken(event.target.value);
                  if (error) setError("");
                }}
                aria-invalid={!!error}
                aria-describedby={error ? "token-error" : "token-hint"}
                className="h-12 rounded-xl border-border bg-background pr-12 pl-11 font-mono text-sm tracking-wide shadow-none placeholder:font-sans placeholder:tracking-normal focus-visible:border-foreground/40 focus-visible:ring-3 focus-visible:ring-foreground/8"
              />
              <Button
                type="button"
                variant="ghost"
                size="icon"
                disabled={busy}
                onClick={() => setShowToken((shown) => !shown)}
                aria-label={showToken ? "Hide token" : "Show token"}
                aria-pressed={showToken}
                className="absolute top-1 right-1 size-10 rounded-lg text-muted-foreground hover:text-foreground"
              >
                {showToken ? (
                  <EyeOffIcon aria-hidden className="size-4" />
                ) : (
                  <EyeIcon aria-hidden className="size-4" />
                )}
              </Button>
            </div>
            {error ? (
              <p
                id="token-error"
                role="alert"
                className="mt-3 flex items-start gap-2 rounded-lg bg-destructive/6 px-3 py-2.5 text-[13px] leading-5 text-destructive"
              >
                <AlertCircleIcon aria-hidden className="mt-0.5 size-4 shrink-0" />
                {error}
              </p>
            ) : (
              <p id="token-hint" className="mt-2.5 text-xs leading-5 text-muted-foreground">
                Use the token shown after installation.
              </p>
            )}
            <Button
              type="submit"
              className="mt-6 h-12 w-full gap-2 rounded-xl text-sm font-medium"
              disabled={busy || !token.trim()}
            >
              {busy ? (
                <>
                  <LoaderCircleIcon
                    aria-hidden
                    className="size-4 animate-spin motion-reduce:animate-none"
                  />
                  Signing in…
                </>
              ) : (
                <>
                  Sign in
                  <ArrowRightIcon aria-hidden className="size-4" />
                </>
              )}
            </Button>
          </form>
          <div className="mt-7 border-t border-border/70 pt-6">
            <h2 className="text-[13px] font-medium">Need your token?</h2>
            <p className="mt-1.5 text-xs leading-5 text-muted-foreground">
              Run this command on the machine where Kyler is installed.
            </p>
            <div className="mt-3 flex items-center gap-3 rounded-xl border border-border/60 bg-muted/50 px-3.5 py-3">
              <TerminalIcon aria-hidden className="size-4 shrink-0 text-muted-foreground" />
              <code className="select-all font-mono text-xs leading-5 text-foreground/85">
                kyler service token
              </code>
            </div>
          </div>
        </section>
      </div>
    </main>
  );
}
