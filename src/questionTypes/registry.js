function createRegistry() {
  const types = [];
  return {
    register(typeModule) {
      types.push(typeModule);
    },
    findHandler(dom) {
      return types.find((t) => t.detect(dom)) || null;
    },
    list() {
      return types.slice();
    },
  };
}

module.exports = { createRegistry };
