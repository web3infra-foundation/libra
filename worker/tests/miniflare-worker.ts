const miniflareWorker = {
  fetch() {
    return new Response("ok");
  },
};

export default miniflareWorker;
