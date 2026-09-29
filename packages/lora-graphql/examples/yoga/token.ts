// Mints a development token with the "editor" role, signed with the same
// secret the server verifies. Real deployments get tokens from their
// identity provider. Usage: npm run token [subject]

import { SignJWT } from "jose";

const secret = new TextEncoder().encode(
  process.env.JWT_SECRET ?? "dev-only-secret-do-not-use",
);

const token = await new SignJWT({ roles: ["editor"] })
  .setProtectedHeader({ alg: "HS256" })
  .setSubject(process.argv[2] ?? "ann")
  .setIssuedAt()
  .setExpirationTime("1h")
  .sign(secret);

console.log(token);
