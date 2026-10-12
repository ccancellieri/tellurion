import{a as e,n as t,t as n}from"./api-DcdVQm8X.js";import{c as r,u as i,x as a}from"./index-BV8t_8Zo.js";var o=`tellurion-png`,s=class extends HTMLElement{#e=null;#t;#n;#r=[];connectedCallback(){this.innerHTML=`
      <div class="panel__controls">
        <label>
          Collection
          <select data-field="collection"></select>
        </label>
      </div>
      <p class="panel__status" data-field="status"></p>
      <div class="panel__map" data-field="map"></div>
    `,this.#t=this.querySelector(`[data-field="collection"]`),this.#n=this.querySelector(`[data-field="status"]`);let e=this.querySelector(`[data-field="map"]`);this.#e=r(e),this.#e.on(`load`,()=>this.#a()),this.#t.addEventListener(`change`,()=>this.#a()),this.#i()}disconnectedCallback(){this.#e?.remove(),this.#e=null}async#i(){try{let t=await e();if(this.#r=t.collections,this.#t.replaceChildren(...this.#r.map(e=>{let t=document.createElement(`option`);return t.value=e.id,t.textContent=e.id,t})),this.#r.length===0){this.#n.textContent=`no collections available`;return}this.#e?.loaded()&&this.#a()}catch(e){this.#n.textContent=e instanceof Error?e.message:String(e)}}#a(){let e=this.#e;if(!e)return;let r=this.#r.find(e=>e.id===this.#t.value);r&&(e.getLayer(`png-raster`)&&e.removeLayer(`png-raster`),e.getSource(o)&&e.removeSource(o),e.addSource(o,{type:`raster`,tiles:[a(``,t,n,r.id,`png`)],tileSize:256}),e.addLayer({id:`png-raster`,type:`raster`,source:o}),i(e,r.extent),this.#n.textContent=`serving PNG tiles for "${r.id}"`)}};customElements.define(`tellurion-png-panel`,s);export{s as TellurionPngPanel};