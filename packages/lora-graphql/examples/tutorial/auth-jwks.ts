import { createRemoteJWKSet, jwtVerify, type JWTPayload } from "jose";

// Your identity provider publishes its public keys at a well-known URL.
// jose fetches them on first use, caches them, and refetches when a token
// names a key it has not seen (key rotation).
const keys = createRemoteJWKSet(
  new URL(
    process.env.JWKS_URL ?? "https://auth.example.com/.well-known/jwks.json",
  ),
);

const issuer = process.env.JWT_ISSUER ?? "https://auth.example.com/";
const audience = process.env.JWT_AUDIENCE ?? "blog-api";

export async function verifiedClaims(
  authorization: string | null | undefined,
): Promise<JWTPayload | undefined> {
  if (!authorization) return undefined;
  const token = /^Bearer (.+)$/.exec(authorization)?.[1];
  if (!token) throw new Error("not a bearer token");
  const { payload } = await jwtVerify(token, keys, {
    issuer,
    audience,
    algorithms: ["RS256", "ES256"],
  });
  return payload;
}
