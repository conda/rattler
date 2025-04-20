import { create_wasm_404_response } from '../js';

describe('create_wasm_404_response', () => {
  it('should create a 404 response with proper headers', async () => {
    const url = 'http://example.com';
    const body = 'Mirror does not support zstd';
    const response = create_wasm_404_response(url, body);

    expect(response.status).toBe(404);
    expect(response.headers.get('Content-Type')).toBe('text/plain');
    expect(response.headers.get('Content-Length')).toBe(String(body.length));
    
    const text = await response.text();
    expect(text).toBe(body);
  });

  it('should throw error for invalid URL', () => {
    expect(() => create_wasm_404_response('invalid-url', 'body')).toThrow();
  });
}); 