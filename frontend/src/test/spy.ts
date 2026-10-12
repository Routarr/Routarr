/**
 * The arguments of a spy's call at `index`, or an error saying how many calls were made.
 *
 * `spy.mock.calls[0][1]` on a missing call fails as "cannot read properties of undefined",
 * which names neither call, and needs a `!` under `noUncheckedIndexedAccess`.
 */
export function nthCall<A extends unknown[]>(spy: { mock: { calls: A[] } }, index = 0): A {
  const call = spy.mock.calls[index];
  if (!call) {
    throw new Error(`expected at least ${index + 1} call(s), saw ${spy.mock.calls.length}`);
  }
  return call;
}
