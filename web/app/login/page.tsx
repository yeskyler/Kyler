import { safeNext } from "@/lib/kyler/web-auth";
import { TokenLogin } from "./token-login";

export const dynamic = "force-dynamic";
export default async function Login({
  searchParams,
}: {
  searchParams: Promise<{ next?: string | string[] }>;
}) {
  return <TokenLogin returnTo={safeNext((await searchParams).next)} />;
}
