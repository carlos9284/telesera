/* Experimental ES5 compatibility layer for Chromium 38 on LG webOS 3.x.
 * This is a browser shim, not a modified webOS system library.
 */
(function (window, document) {
  'use strict';
  if (typeof window.Promise !== 'function') {
    throw new Error('A Promise polyfill is required on this browser');
  }
  if (!Array.from) {
    Array.from = function (value) {
      if (typeof value === 'string') return value.split('');
      return Array.prototype.slice.call(value || []);
    };
  }
  if (!Array.prototype.find) {
    Array.prototype.find = function (callback, thisArg) {
      for (var i = 0; i < this.length; i++) {
        if (callback.call(thisArg, this[i], i, this)) return this[i];
      }
      return undefined;
    };
  }
  // String.prototype.includes was introduced after the Chromium 38 build on LG webOS 3.x.
  // Chat title filtering uses it in ui/app.js.
  if (!String.prototype.includes) {
    String.prototype.includes = function (search, start) {
      if (search instanceof RegExp) throw new TypeError('First argument must not be a RegExp');
      return String(this).indexOf(String(search), start || 0) !== -1;
    };
  }
  if (!Array.prototype.includes) {
    Array.prototype.includes = function (item, start) {
      for (var i = start || 0; i < this.length; i++) {
        if (this[i] === item || (this[i] !== this[i] && item !== item)) return true;
      }
      return false;
    };
  }
  if (!String.prototype.padStart) {
    String.prototype.padStart = function (length, fill) {
      var text = String(this), pad = fill === undefined ? ' ' : String(fill);
      if (!pad) return text;
      while (text.length < length) text = pad + text;
      return text.slice(-length);
    };
  }
  if (!String.prototype.normalize) {
    String.prototype.normalize = function () { return String(this); };
  }
  if (!Number.isFinite) {
    Number.isFinite = function (value) { return typeof value === 'number' && isFinite(value); };
  }
  if (window.NodeList && !NodeList.prototype.forEach) {
    NodeList.prototype.forEach = Array.prototype.forEach;
  }
  if (window.HTMLCollection && !HTMLCollection.prototype.forEach) {
    HTMLCollection.prototype.forEach = Array.prototype.forEach;
  }
  if (window.Element) {
    if (!Element.prototype.remove) {
      Element.prototype.remove = function () { if (this.parentNode) this.parentNode.removeChild(this); };
    }
    if (!Element.prototype.replaceWith) {
      Element.prototype.replaceWith = function (node) {
        if (this.parentNode) this.parentNode.replaceChild(node, this);
      };
    }
    if (!Element.prototype.closest) {
      Element.prototype.closest = function (selector) {
        var element = this;
        while (element && element.nodeType === 1) {
          var match = element.matches || element.webkitMatchesSelector || element.msMatchesSelector;
          if (match && match.call(element, selector)) return element;
          element = element.parentElement;
        }
        return null;
      };
    }
  }
  function decode(s) {
    try { return decodeURIComponent(String(s).replace(/\+/g, ' ')); }
    catch (_) { return String(s); }
  }
  function Params(raw, onChange) {
    this.pairs = [];
    this.changed = onChange || null;
    var items = String(raw || '').replace(/^[?#]/, '').split('&');
    for (var i = 0; i < items.length; i++) {
      if (!items[i]) continue;
      var index = items[i].indexOf('=');
      this.pairs.push([decode(index < 0 ? items[i] : items[i].slice(0, index)),
        decode(index < 0 ? '' : items[i].slice(index + 1))]);
    }
  }
  Params.prototype.get = function (name) {
    name = String(name);
    for (var i = 0; i < this.pairs.length; i++) {
      if (this.pairs[i][0] === name) return this.pairs[i][1];
    }
    return null;
  };
  Params.prototype.has = function (name) { return this.get(name) !== null; };
  Params.prototype.keys = function () {
    return this.pairs.map(function (pair) { return pair[0]; });
  };
  Params.prototype.toString = function () {
    return this.pairs.map(function (pair) {
      return encodeURIComponent(pair[0]) + '=' + encodeURIComponent(pair[1]);
    }).join('&');
  };
  Params.prototype.set = function (name, value) {
    name = String(name);
    var result = [], found = false;
    for (var i = 0; i < this.pairs.length; i++) {
      if (this.pairs[i][0] === name) {
        if (!found) result.push([name, String(value)]);
        found = true;
      } else result.push(this.pairs[i]);
    }
    if (!found) result.push([name, String(value)]);
    this.pairs = result;
    if (this.changed) this.changed(this.toString());
  };
  if (typeof window.URLSearchParams !== 'function') window.URLSearchParams = Params;
  // Old webOS has URL/anchor parsing but no URL.searchParams. Return an anchor URL.
  // Only URL methods used by Telesera are supplied; this is NOT a full URL polyfill.
  if (!('searchParams' in document.createElement('a'))) {
    window.URL = function (raw) {
      var link = document.createElement('a');
      link.href = String(raw);
      link.searchParams = new Params(link.search, function (query) {
        link.search = query ? '?' + query : '';
      });
      if (/^https?:\/\/[^/]*@/i.test(link.href)) {
        // Never allow credentials in web links, even on old DOM implementations.
        link.username = 'blocked';
      }
      return link;
    };
  }
  if (typeof window.AbortController !== 'function') {
    window.AbortController = function () {
      var listeners = [];
      this.signal = {
        aborted: false,
        addEventListener: function (name, cb) { if (name === 'abort') listeners.push(cb); }
      };
      this.abort = function () {
        if (this.signal.aborted) return;
        this.signal.aborted = true;
        for (var i = 0; i < listeners.length; i++) listeners[i]();
      };
    };
  }
  if (typeof window.fetch !== 'function') {
    window.fetch = function (url, options) {
      options = options || {};
      return new Promise(function (resolve, reject) {
        var xhr = new XMLHttpRequest();
        var completed = false;
        function fail(message) {
          if (completed) return;
          completed = true;
          reject(new Error(message));
        }
        xhr.open(options.method || 'GET', url, true);
        var headers = options.headers || {};
        Object.keys(headers).forEach(function (key) { xhr.setRequestHeader(key, headers[key]); });
        if (options.cache === 'no-store') xhr.setRequestHeader('Cache-Control', 'no-store');
        xhr.onload = function () {
          if (completed) return;
          completed = true;
          resolve({
            ok: xhr.status >= 200 && xhr.status < 300,
            status: xhr.status,
            json: function () {
              try { return Promise.resolve(JSON.parse(xhr.responseText)); }
              catch (e) { return Promise.reject(e); }
            }
          });
        };
        xhr.onerror = function () { fail('Network request failed'); };
        xhr.onabort = function () { fail('Aborted'); };
        if (options.signal) {
          if (options.signal.aborted) { fail('Aborted'); return; }
          options.signal.addEventListener('abort', function () { xhr.abort(); });
        }
        try { xhr.send(options.body === undefined ? null : options.body); }
        catch (e) { fail(e.message); }
      });
    };
  }
})(window, document);
