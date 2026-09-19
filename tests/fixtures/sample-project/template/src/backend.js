import { products } from './products.js';

export function listProducts() {
  return { products };
}

export function getProduct(id) {
  return products.find((product) => product.id === id) ?? null;
}
