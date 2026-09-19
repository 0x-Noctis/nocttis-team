import test from 'node:test';
import assert from 'node:assert/strict';
import { getProduct, listProducts } from '../src/backend.js';
import { renderProducts } from '../src/frontend.js';

test('backend lists products and handles missing IDs', () => {
  assert.equal(listProducts().products.length, 2);
  assert.equal(getProduct('lamp').name, 'Moon Lamp');
  assert.equal(getProduct('missing'), null);
});

test('frontend renders product names and prices', () => {
  assert.equal(
    renderProducts([{ name: 'Moon Lamp', price: 24 }]),
    '<ul><li>Moon Lamp: $24.00</li></ul>',
  );
});

test('backend response renders across the stack', () => {
  const page = renderProducts(listProducts().products);
  assert.match(page, /Moon Lamp: \$24\.00/);
  assert.match(page, /Night Mug: \$12\.50/);
});
