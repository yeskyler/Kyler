import { createHash, createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { mkdir, readFile, readdir, unlink, writeFile } from "node:fs/promises";
import { join } from "node:path";

export const sessionSeconds = 7 * 24 * 60 * 60;
const sessionFormat = /^[A-Za-z0-9_-]{43}$/;
const digest = (value: string) => createHash("sha256").update(value).digest("hex");
const proof = (token: string, id: string, expires: number) =>
  createHmac("sha256", token).update(`kyler-web:${id}:${expires}`).digest("hex");

export class WebSessions {
  private readonly directory: string;

  constructor(directory: string) {
    this.directory = directory;
  }

  async create(token: string, now = Date.now()) {
    await mkdir(this.directory, { recursive: true, mode: 0o700 });
    // Expiration is also checked on every authentication request.
    const seconds = Math.floor(now / 1000);
    const names = await readdir(this.directory);
    await Promise.all(
      names
        .filter((file) => /^[a-f0-9]{64}$/.test(file))
        .map(async (file) => {
          const record = await this.read(file);
          if (record && record.expires <= seconds) await this.remove(file);
        }),
    );
    const id = randomBytes(32).toString("base64url");
    const expires = seconds + sessionSeconds;
    await writeFile(
      join(this.directory, digest(id)),
      JSON.stringify({ expires, proof: proof(token, id, expires) }),
      {
        flag: "wx",
        mode: 0o600,
      },
    );
    return id;
  }

  private async read(file: string): Promise<{ expires: number; proof: string } | null> {
    let text: string;
    try {
      text = await readFile(join(this.directory, file), "utf8");
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return null;
      throw error;
    }
    try {
      const value = JSON.parse(text);
      return Number.isSafeInteger(value?.expires) &&
        typeof value?.proof === "string" &&
        /^[a-f0-9]{64}$/.test(value.proof)
        ? value
        : null;
    } catch {
      return null;
    }
  }

  async valid(id: string, token: string, now = Date.now()) {
    if (!token || !sessionFormat.test(id)) return false;
    const record = await this.read(digest(id));
    const seconds = Math.floor(now / 1000);
    return (
      !!record &&
      record.expires > seconds &&
      record.expires <= seconds + sessionSeconds &&
      timingSafeEqual(
        Buffer.from(record.proof, "hex"),
        Buffer.from(proof(token, id, record.expires), "hex"),
      )
    );
  }

  private async remove(file: string) {
    try {
      await unlink(join(this.directory, file));
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }
  }

  async revoke(id: string) {
    if (sessionFormat.test(id)) await this.remove(digest(id));
  }
}
