/**
 * Reading a spy's calls without asserting they happened.
 *
 * `spy.mock.calls[0][1]` is the shape every assertion here wants, and when the
 * call was never made it fails as `cannot read properties of undefined` — which
 * names neither the call that was expected nor the one that was made. Going
 * through this instead turns the same mistake into a sentence, and satisfies
 * `noUncheckedIndexedAccess` without an assertion the compiler cannot check.
 *
 * The returned value is the call's argument *tuple*, so indexing it afterwards
 * is checked by arity rather than left open.
 */
export function nthCall<A extends unknown[]>(spy: { mock: { calls: A[] } }, index = 0): A {
  const call = spy.mock.calls[index];
  if (!call) {
    throw new Error(`expected at least ${index + 1} call(s), saw ${spy.mock.calls.length}`);
  }
  return call;
}
