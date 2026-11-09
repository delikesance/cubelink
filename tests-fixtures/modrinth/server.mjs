#!/usr/bin/env node
// Faux Modrinth : `node tests-fixtures/modrinth/server.mjs --port 0`
import { createServer } from './lib.mjs';

const i = process.argv.indexOf('--port');
const port = i >= 0 ? Number(process.argv[i + 1]) : 0;
const server = createServer();
const { port: p, base } = await server.start(port);
console.log(JSON.stringify({ ready: true, port: p, base }));
const stop = () => server.close(() => process.exit(0));
process.on('SIGINT', stop);
process.on('SIGTERM', stop);
