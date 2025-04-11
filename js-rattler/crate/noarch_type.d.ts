/**
 * Noarch packages are packages that are not architecture specific and therefore
 * only have to be built once. A `NoArchType` is either specific to an
 * architecture or not.
 *
 * @public
 */
export declare type NoArchType = undefined | true | "python" | "generic";
