export function renderProducts(products) {
  const items = products
    .map(({ name, price }) => `<li>${name}: $${price.toFixed(2)}</li>`)
    .join('');
  return `<ul>${items}</ul>`;
}
