import { jwtVerify, type JWTPayload } from "jose";

const secret = new TextEncoder().encode(
  process.env.JWT_SECRET ?? "dev-only-secret-do-not-use",
);

export const issuer = process.env.JWT_ISSUER ?? "https://auth.example.com/";
export const audience = process.env.JWT_AUDIENCE ?? "blog-api";

/**
 * The verified claims of a request. Undefined without a token; throws for
 * a token that is expired, forged, malformed or meant for someone else.
 */
export async function verifiedClaims(
  authorization: string | null | undefined,
): Promise<JWTPayload | undefined> {
  if (!authorization) return undefined;
  const token = /^Bearer (.+)$/.exec(authorization)?.[1];
  if (!token) throw new Error("not a bearer token");
  const { payload } = await jwtVerify(token, secret, {
    issuer,
    audience,
    algorithms: ["HS256"],
  });
  return payload;
}
