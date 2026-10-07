// Starts each server in this directory and calls it the way a client
// would: over HTTP and WebSocket, with tokens signed like real ones.
// Run: npm test

import assert from "node:assert/strict";
import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import { createServer } from "node:http";
import { after, describe, test } from "node:test";
import { createClient } from "graphql-ws";
import { SignJWT, exportJWK, generateKeyPair } from "jose";
import WebSocket from "ws";
import { audience, issuer } from "./auth.js";

const secret = new TextEncoder().encode("dev-only-secret-do-not-use");

function token(
  subject: string,
  options: { expiresIn?: string; audience?: string; key?: Uint8Array } = {},
) {
  return new SignJWT({ roles: [] })
    .setProtectedHeader({ alg: "HS256" })
    .setSubject(subject)
    .setIssuer(issuer)
    .setAudience(options.audience ?? audience)
    .setIssuedAt()
    .setExpirationTime(options.expiresIn ?? "5m")
    .sign(options.key ?? secret);
}

const children: ChildProcess[] = [];
after(() => {
  for (const child of children) child.kill();
});

/** Runs a server file on a free port and resolves once it listens. */
async function start(file: string, port: number): Promise<string> {
  const child = spawn(process.execPath, ["--import", "tsx", file], {
    env: { ...process.env, PORT: String(port), NODE_ENV: "development" },
    stdio: ["ignore", "pipe", "inherit"],
  });
  children.push(child);
  await new Promise<void>((resolve, reject) => {
    child.stdout!.on("data", (chunk: Buffer) => {
      if (chunk.toString().includes("listening")) resolve();
    });
    child.on("exit", (code) => reject(new Error(`${file} exited (${code})`)));
  });
  return `http://localhost:${port}`;
}

interface Answer {
  status: number;
  data?: Record<string, unknown> | null;
  errors?: Array<{ message: string; extensions?: { code?: string } }>;
}

async function post(
  url: string,
  query: string,
  authorization?: string,
): Promise<Answer> {
  const response = await fetch(url, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      ...(authorization ? { authorization } : {}),
    },
    body: JSON.stringify({ query }),
  });
  const text = await response.text();
  const body = text ? (JSON.parse(text) as Omit<Answer, "status">) : {};
  return { status: response.status, ...body };
}

const POSTS = `{ posts(sort: [{ slug: ASC }]) { slug } }`;
const slugs = (answer: Answer) =>
  (answer.data?.posts as Array<{ slug: string }>).map((p) => p.slug);

// The same contract, whichever server answers.
const servers = [
  { file: "server.ts", port: 4101, path: "/graphql" },
  { file: "apollo.ts", port: 4102, path: "/" },
  { file: "handler.ts", port: 4103, path: "/" },
];

for (const { file, port, path } of servers) {
  describe(file, async () => {
    const url = (await start(file, port)) + path;

    test("an anonymous caller reads published posts only", async () => {
      const answer = await post(url, POSTS);
      assert.equal(answer.status, 200);
      assert.deepEqual(slugs(answer), ["cobol", "engines"]);
    });

    test("a signed-in author also reads their own draft", async () => {
      const answer = await post(url, POSTS, `Bearer ${await token("ada")}`);
      assert.deepEqual(slugs(answer), ["cobol", "engines", "notes"]);
    });

    test("an anonymous write is UNAUTHENTICATED", async () => {
      const answer = await post(
        url,
        `mutation { deletePost(slug: "cobol") { nodesDeleted } }`,
      );
      assert.equal(answer.errors?.[0]?.extensions?.code, "UNAUTHENTICATED");
    });

    test("an author cannot write in someone else's name", async () => {
      const answer = await post(
        url,
        `mutation { createPosts(input: [{ slug: "x", title: "X", author: { connect: { key: "grace" } } }]) { info { nodesCreated } } }`,
        `Bearer ${await token("ada")}`,
      );
      assert.equal(answer.errors?.[0]?.extensions?.code, "FORBIDDEN");
    });

    // A bad token is rejected outright, never treated as anonymous.
    for (const [name, make] of [
      ["an expired token", () => token("ada", { expiresIn: "-1m" })],
      ["a token for another audience", () => token("ada", { audience: "x" })],
      [
        "a token signed with another key",
        () => token("ada", { key: new TextEncoder().encode("not-the-secret") }),
      ],
      ["a malformed token", async () => "not.a.token"],
    ] as const) {
      test(`${name} is a 401`, async () => {
        const answer = await post(url, POSTS, `Bearer ${await make()}`);
        assert.equal(answer.status, 401);
        assert.equal(answer.data?.posts, undefined);
      });
    }
  });
}

describe("ws.ts", async () => {
  const base = await start("ws.ts", 4104);
  const http = `${base}/graphql`;
  const ws = http.replace("http", "ws");

  test("a subscriber receives the post an author creates", async () => {
    const client = createClient({
      url: ws,
      webSocketImpl: WebSocket,
      connectionParams: { authorization: `Bearer ${await token("ada")}` },
    });
    const events = client.iterate<{
      postChanged: { operation: string; slug: string };
    }>({ query: `subscription { postChanged { operation slug } }` });
    const first = events.next();
    // Give the subscription a moment to register before writing.
    await new Promise((resolve) => setTimeout(resolve, 300));
    const created = await post(
      http,
      `mutation { createPosts(input: [{ slug: "live", title: "Live", author: { connect: { key: "ada" } } }]) { info { nodesCreated } } }`,
      `Bearer ${await token("ada")}`,
    );
    assert.equal(created.errors, undefined);
    const { value } = await first;
    assert.deepEqual(value?.data?.postChanged, {
      operation: "CREATE",
      slug: "live",
    });
    await client.dispose();
  });

  test("an anonymous subscription is UNAUTHENTICATED", async () => {
    const client = createClient({ url: ws, webSocketImpl: WebSocket });
    const events = client.iterate({
      query: `subscription { postChanged { slug } }`,
    });
    const { value } = await events.next();
    assert.equal(value?.errors?.[0]?.extensions?.code, "UNAUTHENTICATED");
    await client.dispose();
  });

  test("a bad token cannot open a connection", async () => {
    const client = createClient({
      url: ws,
      webSocketImpl: WebSocket,
      connectionParams: { authorization: "Bearer not.a.token" },
      retryAttempts: 0,
    });
    const events = client.iterate({
      query: `subscription { postChanged { slug } }`,
    });
    await assert.rejects(() => events.next());
    await client.dispose();
  });
});

describe("auth-jwks.ts", async () => {
  // Stand in for the identity provider: a key pair, and its public key
  // served as a JWKS document.
  const { publicKey, privateKey } = await generateKeyPair("RS256");
  const jwk = {
    ...(await exportJWK(publicKey)),
    kid: "test-key",
    alg: "RS256",
  };
  const provider = createServer((_, res) => {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ keys: [jwk] }));
  });
  await new Promise<void>((resolve) => provider.listen(4105, resolve));
  after(() => provider.close());

  process.env.JWKS_URL = "http://localhost:4105/jwks.json";
  const { verifiedClaims } = await import("./auth-jwks.js");

  const signed = (aud: string) =>
    new SignJWT({ roles: ["member"] })
      .setProtectedHeader({ alg: "RS256", kid: "test-key" })
      .setSubject("ada")
      .setIssuer(issuer)
      .setAudience(aud)
      .setExpirationTime("5m")
      .sign(privateKey);

  test("verifies a token against the provider's published key", async () => {
    const claims = await verifiedClaims(`Bearer ${await signed(audience)}`);
    assert.equal(claims?.sub, "ada");
    assert.deepEqual(claims?.roles, ["member"]);
  });

  test("no header is anonymous; a wrong audience throws", async () => {
    assert.equal(await verifiedClaims(undefined), undefined);
    await assert.rejects(async () =>
      verifiedClaims(`Bearer ${await signed("someone-else")}`),
    );
  });
});

describe("tooling", () => {
  test("token.ts mints a token auth.ts accepts", async () => {
    const minted = execFileSync(
      process.execPath,
      ["--import", "tsx", "token.ts", "grace"],
      { encoding: "utf8" },
    ).trim();
    const { verifiedClaims } = await import("./auth.js");
    assert.equal((await verifiedClaims(`Bearer ${minted}`))?.sub, "grace");
  });

  test("lora-graphql check accepts the schema", () => {
    execFileSync("npx", ["lora-graphql", "check", "schema.graphql"], {
      stdio: "pipe",
    });
  });
});
