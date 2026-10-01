// umi-downloader 插件（内置示例）：直链嗅探器
// 能力面：只使用 umi.resolve / umi.registerResolver / umi.export / umi.log
// 作用：宿主给出 URL 的结构化分析（扩展名、主机、是否直链、置信度），
//       插件据此给出「这条链接能否直接交给下载引擎」的意见。
(function () {
  "use strict";

  // 把 umi.resolve 的结论翻译成解析器意见
  function sniff(url) {
    var info = umi.resolve(url);
    if (!info || info.ok !== true) {
      return {
        matched: false,
        direct: false,
        url: url,
        kind: "unknown",
        confidence: 0,
        reason: "不是可解析的绝对 URL：" + ((info && info.reason) || "unknown")
      };
    }
    if (info.direct === true) {
      return {
        matched: true,
        direct: true,
        url: url,
        kind: info.kind,
        confidence: info.confidence,
        reason: "直链特征命中（" + info.reason + "）"
      };
    }
    return {
      matched: true,
      direct: false,
      url: url,
      kind: info.kind,
      confidence: info.confidence,
      reason: "疑似网页/接口，需交给站点解析器：" + info.reason
    };
  }

  umi.registerResolver(sniff);

  umi.export("selfTest", function () {
    var media = sniff("https://cdn.example.com/media/movie-1080p.mp4");
    var page = sniff("https://example.com/watch?v=12345");
    var archive = sniff("https://example.com/download/file.zip?token=abc");
    var p2p = sniff("magnet:?xt=urn:btih:0123456789abcdef");
    umi.log("selfTest: mp4.direct=" + media.direct + " page.direct=" + page.direct +
            " zip.direct=" + archive.direct + " magnet.direct=" + p2p.direct);
    if (media.direct !== true) return false;
    if (page.direct !== false) return false;
    if (archive.direct !== true) return false;
    if (p2p.direct !== true) return false;
    if (!(media.confidence > 0.5)) return false;
    return true;
  });

  umi.log("direct-link-sniffer 就绪（umi " + umi.version + "）");
})();
