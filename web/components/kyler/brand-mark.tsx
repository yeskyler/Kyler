import Image from "next/image";

export function BrandMark({ size = 32 }: { size?: number }) {
  return (
    <Image
      src="/icon.svg"
      alt=""
      width={size}
      height={size}
      unoptimized
      className="shrink-0 rounded-[25%] ring-1 ring-foreground/10 dark:ring-foreground/20"
    />
  );
}
