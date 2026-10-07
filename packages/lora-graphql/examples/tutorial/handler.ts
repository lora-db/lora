// No GraphQL server library: node:http and lora.execute(), which applies
// the document guards, caches parsed documents and reports cost itself.
// Run: npx tsx handler.ts

import { createServer } from "node:http";
import { verifiedClaims } from "./auth.js";
import { lora } from "./lora.js";

const server = createServer(async (req, res) => {
  let jwt;
  try {
    jwt = await verifiedClaims(req.headers.authorization);
  } catch {
    res.writeHead(401, { "www-authenticate": "Bearer" }).end();
    return;
  }

  let body = "";
  for await (const chunk of req) body += chunk;
  const { query, variables, operationName } = JSON.parse(body);

  // Stop the request's statements when the client goes away.
  const controller = new AbortController();
  res.on("close", () => controller.abort());

  const result = await lora.execute({
    source: query,
    variables,
    operationName,
    context: { jwt, signal: controller.signal },
  });
  res.writeHead(200, { "content-type": "application/json" });
  res.end(JSON.stringify(result));
});

const port = Number(process.env.PORT ?? 4000);
server.listen(port, () => {
  console.log(`listening on http://localhost:${port}/`);
});
