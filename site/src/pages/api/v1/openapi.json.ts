import type { APIRoute } from 'astro';
import contract from '../../../../../backend/openapi/v1.json?raw';

/**
 * The contract, byte for byte, where the API page's button and a client's
 * import find it. It is the path every Routarr serves it at, so an address
 * written for one works for the other.
 */
export const GET: APIRoute = () =>
  new Response(contract, { headers: { 'Content-Type': 'application/json' } });
