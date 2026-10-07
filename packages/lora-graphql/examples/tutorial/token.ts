// Mints a development token, signed with the secret auth.ts verifies.
// Real deployments get tokens from their identity provider.
// Usage: npm run token -- <subject> [role...]

import { SignJWT } from "jose";
import { audience, issuer } from "./auth.js";

const secret = new TextEncoder().encode(
  process.env.JWT_SECRET ?? "dev-only-secret-do-not-use",
);
const [subject = "ada", ...roles] = process.argv.slice(2);

console.log(
  await new SignJWT({ roles })
    .setProtectedHeader({ alg: "HS256" })
    .setSubject(subject)
    .setIssuer(issuer)
    .setAudience(audience)
    .setIssuedAt()
    .setExpirationTime("1h")
    .sign(secret),
);
