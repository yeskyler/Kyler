"use client";

import {
  createContext,
  useContext,
  useRef,
  useState,
  type ComponentProps,
  type ClipboardEvent,
} from "react";
import { ComposerPrimitive, useAui } from "@assistant-ui/react";
import { AlertCircleIcon, FileTextIcon, LoaderCircleIcon, XIcon } from "lucide-react";
import { attachPastedText, shouldAttachPastedText } from "@/lib/kyler/pasted-document";

const PasteContext = createContext({
  busy: false,
  error: "",
  begin: () => () => {},
  notice: (_text: string) => {},
});

export function PasteComposerRoot({
  children,
  ...props
}: ComponentProps<typeof ComposerPrimitive.Root>) {
  const pending = useRef(0);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState("");
  return (
    <PasteContext.Provider
      value={{
        busy,
        error: notice,
        notice: setNotice,
        begin: () => {
          pending.current++;
          setBusy(true);
          setNotice("");
          return () => {
            pending.current--;
            setBusy(pending.current > 0);
          };
        },
      }}
    >
      <ComposerPrimitive.Root
        {...props}
        onSubmitCapture={(event) => {
          props.onSubmitCapture?.(event);
          if (pending.current) {
            event.preventDefault();
            event.stopPropagation();
          }
        }}
        onKeyDownCapture={(event) => {
          props.onKeyDownCapture?.(event);
          if (pending.current && event.key === "Enter" && !event.shiftKey) {
            event.preventDefault();
            event.stopPropagation();
          }
        }}
      >
        {children}
      </ComposerPrimitive.Root>
    </PasteContext.Provider>
  );
}

export function PasteComposerInput(props: ComponentProps<typeof ComposerPrimitive.Input>) {
  const aui = useAui();
  const paste = useContext(PasteContext);
  function onPaste(event: ClipboardEvent<HTMLTextAreaElement>) {
    props.onPaste?.(event);
    if (event.defaultPrevented) return;
    const text = event.clipboardData.getData("text/plain");
    if (!shouldAttachPastedText(text, event.clipboardData.files.length > 0)) return;
    event.preventDefault();
    const composer = aui.composer();
    const start = event.currentTarget.selectionStart;
    const end = event.currentTarget.selectionEnd;
    const finish = paste.begin();
    void (async () => {
      try {
        const attached = await attachPastedText(composer, text, start, end);
        if (!attached)
          paste.notice(
            "The Word attachment could not be uploaded. Pasted text was restored to the message.",
          );
      } finally {
        finish();
      }
    })();
  }
  return <ComposerPrimitive.Input {...props} onPaste={onPaste} />;
}

export function PasteComposerSend(props: ComponentProps<typeof ComposerPrimitive.Send>) {
  const { busy } = useContext(PasteContext);
  return <ComposerPrimitive.Send {...props} disabled={busy || props.disabled} />;
}

export function PasteAttachmentStatus() {
  const { busy, error, notice } = useContext(PasteContext);
  if (!busy && !error) return null;
  const label = busy ? "Uploading pasted text as a Word attachment" : error;
  const Icon = busy ? LoaderCircleIcon : AlertCircleIcon;
  return (
    <div
      className="relative shrink-0"
      role={busy ? "status" : "alert"}
      aria-label={label}
      title={label}
    >
      <div
        className={`relative flex size-14 items-center justify-center rounded-lg bg-muted ring-1 ${busy ? "ring-border" : "ring-destructive/60"}`}
      >
        <FileTextIcon aria-hidden className="size-6 text-muted-foreground/80" />
        <span
          aria-hidden
          className="absolute bottom-0.5 right-0.5 rounded-full bg-background p-0.5"
        >
          <Icon
            className={`size-3.5 ${busy ? "animate-spin text-muted-foreground motion-reduce:animate-none" : "text-destructive"}`}
          />
        </span>
      </div>
      {!busy && (
        <button
          type="button"
          onClick={() => notice("")}
          aria-label="Dismiss upload error"
          className="absolute right-1 top-1 flex size-5 items-center justify-center rounded-full bg-black/50 text-white hover:bg-black/70"
        >
          <XIcon aria-hidden className="size-3" />
        </button>
      )}
    </div>
  );
}
