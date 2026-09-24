import{a as e,n as t,t as n}from"./api-DcdVQm8X.js";import{C as r,T as i,f as a,u as o}from"./index-CeHZZbQB.js";i();var s=`tellurion-png`,c=class extends HTMLElement{#e=null;#t;#n;#r=[];connectedCallback(){this.innerHTML=`
      <div class="panel__controls">
        <label>
          Collection
          <select data-field="collection"></select>
        </label>
      </div>
      <p class="panel__status" data-field="status"></p>
      <div class="panel__map" data-field="map"></div>
    `,this.#t=this.querySelector(`[data-field="collection"]`),this.#n=this.querySelector(`[data-field="status"]`);let e=this.querySelector(`[data-field="map"]`);this.#e=o(e),this.#e.on(`load`,()=>this.#a()),this.#t.addEventListener(`change`,()=>this.#a()),this.#i()}disconnectedCallback(){this.#e?.remove(),this.#e=null}async#i(){try{let t=await e();if(this.#r=t.collections,this.#t.replaceChildren(...this.#r.map(e=>{let t=document.createElement(`option`);return t.value=e.id,t.textContent=e.id,t})),this.#r.length===0){this.#n.textContent=`no collections available`;return}this.#e?.loaded()&&this.#a()}catch(e){this.#n.textContent=e instanceof Error?e.message:String(e)}}#a(){let e=this.#e;if(!e)return;let i=this.#r.find(e=>e.id===this.#t.value);i&&(e.getLayer(`png-raster`)&&e.removeLayer(`png-raster`),e.getSource(s)&&e.removeSource(s),e.addSource(s,{type:`raster`,tiles:[r(``,t,n,i.id,`png`)],tileSize:256}),e.addLayer({id:`png-raster`,type:`raster`,source:s}),a(e,i.extent),this.#n.textContent=`serving PNG tiles for "${i.id}"`)}};customElements.define(`tellurion-png-panel`,c);export{c as TellurionPngPanel};