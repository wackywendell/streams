import { createHash } from 'node:crypto';

// Match Session::id_for, including Python-compatible ASCII JSON identity strings.
export function sessionId(host,harness,native) {
 const ascii=value=>JSON.stringify(value).replace(/[\u007f-\uffff]/g,c=>'\\u'+c.charCodeAt(0).toString(16).padStart(4,'0'));
 const identity='streams:session:['+[host,harness,native].map(ascii).join(', ')+']';
 const bytes=createHash('sha1').update(Buffer.from('6ba7b8119dad11d180b400c04fd430c8','hex')).update(identity).digest().subarray(0,16);
 bytes[6]=(bytes[6]&15)|80;bytes[8]=(bytes[8]&63)|128;
 const hex=bytes.toString('hex');
 return [hex.slice(0,8),hex.slice(8,12),hex.slice(12,16),hex.slice(16,20),hex.slice(20)].join('-');
}
