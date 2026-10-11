/**
 * Fails the run when the fake Radarr was asked something it does not serve.
 * A real Radarr answers it, and the 404 sends Routarr down the path it takes
 * when Radarr fails, which the suite then exercises in place of the ordinary
 * one without a word.
 */
export default async function teardown(): Promise<void> {
  const arr = process.env.ROUTARR_E2E_ARR ?? 'http://127.0.0.1:7979';
  const unknown = (await (await fetch(`${arr}/__unknown`)).json()) as string[];
  if (unknown.length > 0) {
    throw new Error(`The fake Radarr was asked what it does not serve: ${unknown.join(', ')}`);
  }
}
