import{a as e,c as t,l as n,n as r,t as i}from"./api-DcdVQm8X.js";import{S as a,T as o,f as s,u as c}from"./index-CeHZZbQB.js";o();var l=`tellurion-styled`,u=class extends HTMLElement{#e=null;#t;#n;#r;#i;#a=[];#o=[];connectedCallback(){this.innerHTML=`
      <div class="panel__controls">
        <label>
          Collection
          <select data-field="collection"></select>
        </label>
        <label>
          Style
          <select data-field="style"></select>
        </label>
      </div>
      <p class="panel__status" data-field="status"></p>
      <p class="panel__style-name" data-field="style-name"></p>
      <div class="panel__map" data-field="map"></div>
    `,this.#t=this.querySelector(`[data-field="collection"]`),this.#n=this.querySelector(`[data-field="style"]`),this.#r=this.querySelector(`[data-field="status"]`),this.#i=this.querySelector(`[data-field="style-name"]`);let e=this.querySelector(`[data-field="map"]`);this.#e=c(e),this.#e.on(`load`,()=>void this.#c()),this.#t.addEventListener(`change`,()=>void this.#c()),this.#n.addEventListener(`change`,()=>void this.#c()),this.#s()}disconnectedCallback(){this.#e?.remove(),this.#e=null}async#s(){try{let[t,r]=await Promise.all([e(),n()]);if(this.#a=t.collections,this.#o=r.styles.map(e=>e.id),this.#t.replaceChildren(...this.#a.map(e=>{let t=document.createElement(`option`);return t.value=e.id,t.textContent=e.id,t})),this.#n.replaceChildren(...this.#o.map(e=>{let t=document.createElement(`option`);return t.value=e,t.textContent=e,t})),this.#a.length===0||this.#o.length===0){this.#r.textContent=`no collections or no registered styles available`;return}this.#e?.loaded()&&await this.#c()}catch(e){this.#r.textContent=e instanceof Error?e.message:String(e)}}async#c(){let e=this.#e;if(!e)return;let n=this.#a.find(e=>e.id===this.#t.value),o=this.#n.value;if(!(!n||!o))try{let c=await t(o);this.#i.textContent=typeof c.name==`string`?`style: ${c.name}`:`style: ${o}`,e.getLayer(`styled-raster`)&&e.removeLayer(`styled-raster`),e.getSource(l)&&e.removeSource(l),e.addSource(l,{type:`raster`,tiles:[a(``,r,i,n.id,o)],tileSize:256}),e.addLayer({id:`styled-raster`,type:`raster`,source:l}),s(e,n.extent),this.#r.textContent=`serving "${o}"-styled tiles for "${n.id}"`}catch(e){this.#r.textContent=e instanceof Error?e.message:String(e)}}};customElements.define(`tellurion-styled-panel`,u);export{u as TellurionStyledPanel};