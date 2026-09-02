var e=/^([a-zA-Z_:][a-zA-Z0-9_:]*)(\{[^}]*\})?\s+([0-9eE+\-.]+)$/;function t(t){let n=0,r=0,i=null;for(let a of t.split(`
`)){let t=a.trim();if(t===``||t.startsWith(`#`))continue;let o=e.exec(t);if(!o)continue;let s=o[1],c=Number(o[3]);if(Number.isFinite(c))switch(s){case`http_request_duration_seconds_count`:n+=c;break;case`http_request_duration_seconds_sum`:r+=c;break;case`process_resident_memory_bytes`:i=c;break;default:break}}return{totalRequests:n,avgLatencyMs:n>0?r/n*1e3:null,residentMemoryBytes:i}}var n=class extends HTMLElement{#e=null;connectedCallback(){this.innerHTML=`
      <dl class="status-widget__stats">
        <div class="status-widget__stat">
          <dt>Requests served</dt>
          <dd data-field="requests">&mdash;</dd>
        </div>
        <div class="status-widget__stat">
          <dt>Avg latency</dt>
          <dd data-field="latency">&mdash;</dd>
        </div>
        <div class="status-widget__stat">
          <dt>Resident memory</dt>
          <dd data-field="rss">&mdash;</dd>
        </div>
      </dl>
      <p class="status-widget__error" data-field="error" hidden></p>
    `,this.#t(),this.#e=setInterval(()=>void this.#t(),5e3)}disconnectedCallback(){this.#e!==null&&(clearInterval(this.#e),this.#e=null)}async#t(){try{let e=await fetch(`/metrics`);if(!e.ok)throw Error(`GET /metrics failed: ${e.status} ${e.statusText}`);let n=await e.text();this.#n(t(n)),this.#r(null)}catch(e){this.#r(e instanceof Error?e.message:String(e))}}#n(e){this.#i(`requests`).textContent=e.totalRequests.toFixed(0),this.#i(`latency`).textContent=e.avgLatencyMs===null?`n/a`:`${e.avgLatencyMs.toFixed(1)} ms`,this.#i(`rss`).textContent=e.residentMemoryBytes===null?`n/a (not reported on this platform)`:`${(e.residentMemoryBytes/(1024*1024)).toFixed(1)} MB`}#r(e){let t=this.#i(`error`);t.hidden=e===null,t.textContent=e??``}#i(e){let t=this.querySelector(`[data-field="${e}"]`);if(!t)throw Error(`status widget is missing its "${e}" field`);return t}};customElements.define(`tellurion-status-widget`,n);export{n as TellurionStatusWidget};